#![allow(confusable_idents)]

use clap::Parser;

mod output;
mod sampler;

use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use rayon::current_num_threads;
use rand::Rng;
use sampler::cubebinsampler::{filter_sparse_cells, CubeBinSampler};
use sampler::density::estimate_transcript_density;
use sampler::hull::compute_cell_areas;
use sampler::transcripts::{
    coordinate_span, estimate_cell_centroids, estimate_full_area, filter_cellfree_transcripts,
    read_transcripts_csv, Transcript, BACKGROUND_CELL,
};
use sampler::{ModelParams, ModelPriors, ProposalStats, Sampler, UncertaintyTracker};
use std::cell::RefCell;

use output::*;

use sampler::polyagamma::PolyaGamma;

#[derive(Parser, Debug)]
#[command(name = "proseg")]
#[command(author = "Daniel C. Jones")]
#[command(
    about = "High-speed cell segmentation of transcript-resolution spatial transcriptomics data."
)]
struct Args {
    transcript_csv: String,
    // TODO: We also want to be able to read from xenium parquet files.

    #[arg(long, default_value_t=false)]
    xenium: bool,

    #[arg(long, default_value_t=false)]
    cosmx: bool,

    #[arg(long, default_value_t=false)]
    cosmx_micron: bool,

    #[arg(long, default_value_t=false)]
    merfish: bool,

    #[arg(long, default_value = None)]
    transcript_column: Option<String>,

    #[arg(long, default_value = None)]
    transcript_id_column: Option<String>,

    #[arg(short, long, default_value = None)]
    x_column: Option<String>,

    #[arg(short, long, default_value = None)]
    y_column: Option<String>,

    #[arg(short, long, default_value = None)]
    z_column: Option<String>,

    #[arg(long, default_value = None)]
    compartment_column: Option<String>,

    #[arg(long, default_value = None)]
    compartment_nuclear: Option<String>,

    #[arg(long, default_value = None)]
    fov_column: Option<String>,

    #[arg(long, default_value = None)]
    cell_id_column: Option<String>,

    #[arg(long, default_value = None)]
    cell_id_unassigned: Option<String>,

    #[arg(long, default_value = None)]
    qv_column: Option<String>,

    #[arg(long, default_value_t)]
    ignore_z_coord: bool,

    #[arg(long, default_value_t = 0.0_f32)]
    min_qv: f32,

    #[arg(long, default_value_t = 100)]
    cells_per_chunk: usize,

    #[arg(long, default_value_t = 10)]
    ncomponents: usize,

    #[arg(long, default_value_t = 10)]
    nlayers: usize,

    #[arg(long, default_value_t = 8)]
    voxellayers: usize,

    #[arg(long, num_args=1.., value_delimiter=',', default_values_t=[150, 150, 300])]
    schedule: Vec<usize>,

    #[arg(long, default_value_t=100)]
    recorded_samples: usize,

    #[arg(short = 't', long, default_value=None)]
    nthreads: Option<usize>,

    #[arg(short, long, default_value_t = 1000)]
    local_steps_per_iter: usize,

    #[arg(long, default_value_t = 0.1)]
    count_pr_cutoff: f32,

    #[arg(long, default_value_t = 0.9)]
    foreground_pr_cutoff: f32,

    #[arg(long, default_value_t = 1.4_f32)]
    perimeter_bound: f32,

    #[arg(long, default_value_t = 5e-2_f32)]
    nuclear_reassignment_prob: f32,

    #[arg(long, default_value_t = 5e-1_f32)]
    prior_seg_reassignment_prob: f32,

    // TODO: We need a microns-per-pixel argument. We have a bunch of priors
    // that are basically assuming microns.

    #[arg(long, default_value_t = 4.0_f32)]
    scale: f32,

    #[arg(long, default_value_t = 60_f32)]
    max_transcript_nucleus_distance: f32,

    #[arg(long, default_value_t = false)]
    calibrate_scale: bool,

    #[arg(long, default_value_t = false)]
    no_diffusion: bool,

    #[arg(long, default_value_t = 0.25)]
    diffusion_probability: f32,

    #[arg(long, default_value_t = 20.0)]
    diffusion_proposal_sigma: f32,

    #[arg(long, default_value_t = 8.0)]
    diffusion_sigma_near: f32,

    #[arg(long, default_value_t = 80.0)]
    diffusion_sigma_far: f32,

    #[arg(long, default_value_t = 50.0_f32)]
    density_binsize: f32,

    #[arg(long, default_value_t = 150.0_f32)]
    density_sigma: f32,

    #[arg(long, default_value_t = 10)]
    density_k: usize,

    #[arg(long, default_value_t = 1e-2)]
    density_eps: f32,

    #[arg(long, default_value_t = false)]
    variable_burnin_dispersion: bool,

    #[arg(long, default_value = None)]
    dispersion: Option<f32>,

    #[arg(long, default_value_t = false)]
    check_consistency: bool,

    #[arg(long, default_value = None)]
    output_counts: Option<String>,

    #[arg(long, default_value = None)]
    output_counts_fmt: Option<String>,

    #[arg(long, default_value = "expected-counts.csv.gz")]
    output_expected_counts: Option<String>,

    #[arg(long, default_value = None)]
    output_rates: Option<String>,

    #[arg(long, default_value = None)]
    output_rates_fmt: Option<String>,

    #[arg(long, default_value = None)]
    output_component_params: Option<String>,

    #[arg(long, default_value = None)]
    output_component_params_fmt: Option<String>,

    #[arg(long, default_value = None)]
    output_expected_counts_fmt: Option<String>,

    #[arg(long, default_value = None)]
    output_cell_hulls: Option<String>,

    #[arg(long, default_value = "cell-metadata.csv.gz")]
    output_cell_metadata: Option<String>,

    #[arg(long, default_value = None)]
    output_cell_metadata_fmt: Option<String>,

    #[arg(long, default_value = "transcript-metadata.csv.gz")]
    output_transcript_metadata: Option<String>,

    #[arg(long, default_value=None)]
    output_transcript_metadata_fmt: Option<String>,

    #[arg(long, default_value=None)]
    output_gene_metadata: Option<String>,

    #[arg(long, default_value=None)]
    output_gene_metadata_fmt: Option<String>,

    #[arg(long, default_value=None)]
    output_cell_cubes: Option<String>,

    #[arg(long, default_value=None)]
    output_cell_cubes_fmt: Option<String>,

    // #[arg(long, default_value="cell-polygons.geojson.gz")]
    #[arg(long, default_value=None)]
    output_cell_polygons: Option<String>,

    #[arg(long, default_value=None)]
    output_cell_polygon_layers: Option<String>,

    #[arg(long, default_value = None)]
    monitor_cell_polygons: Option<String>,

    #[arg(long, default_value_t = 10)]
    monitor_cell_polygons_freq: usize,
}

fn set_xenium_presets(args: &mut Args) {
    args.transcript_column.get_or_insert(String::from("feature_name"));
    args.transcript_id_column.get_or_insert(String::from("transcript_id"));
    args.x_column.get_or_insert(String::from("x_location"));
    args.y_column.get_or_insert(String::from("y_location"));
    args.z_column.get_or_insert(String::from("z_location"));
    args.compartment_column.get_or_insert(String::from("overlaps_nucleus"));
    args.compartment_nuclear.get_or_insert(String::from("1"));
    args.cell_id_column.get_or_insert(String::from("cell_id"));
    args.cell_id_unassigned.get_or_insert(String::from("UNASSIGNED"));
    args.qv_column.get_or_insert(String::from("qv"));

    // Xenium coordinates are in microns.
    args.scale = 4.0;

    // TODO: This is not a good thing to be doing, but I'm finding that I need
    // to force the dispersion up to get good results on some of the data.
    // args.dispersion.get_or_insert(40.0);
    // args.dispersion.get_or_insert(20.0);
    // args.dispersion.get_or_insert(10.0);
}


fn set_cosmx_presets(args: &mut Args) {
    args.transcript_column.get_or_insert(String::from("target"));
    args.x_column.get_or_insert(String::from("x_global_px"));
    args.y_column.get_or_insert(String::from("y_global_px"));
    args.z_column.get_or_insert(String::from("z"));
    args.compartment_column.get_or_insert(String::from("CellComp"));
    args.compartment_nuclear.get_or_insert(String::from("Nuclear"));
    args.fov_column.get_or_insert(String::from("fov"));
    args.cell_id_column.get_or_insert(String::from("cell_ID"));
    args.cell_id_unassigned.get_or_insert(String::from("0"));

    // CosmX coordinates are in pixels. (TODO: Where can I find the px per micron)
    args.scale = 20.0;

    // TODO: Because the scale is different, we need to set different parameters
    // for max_nucleaus_transcript_distance, density_sigma, density_binsize
}


fn set_cosmx_micron_presets(args: &mut Args) {
    args.transcript_column.get_or_insert(String::from("target"));
    args.x_column.get_or_insert(String::from("x"));
    args.y_column.get_or_insert(String::from("y"));
    args.z_column.get_or_insert(String::from("z"));
    args.compartment_column.get_or_insert(String::from("CellComp"));
    args.compartment_nuclear.get_or_insert(String::from("Nuclear"));
    args.fov_column.get_or_insert(String::from("fov"));
    args.cell_id_column.get_or_insert(String::from("cell_ID"));
    args.cell_id_unassigned.get_or_insert(String::from("0"));

    args.scale = 4.0;
}

fn set_merfish_presets(args: &mut Args) {
    args.transcript_column.get_or_insert(String::from("gene"));
    args.x_column.get_or_insert(String::from("x"));
    args.y_column.get_or_insert(String::from("y"));
    args.z_column.get_or_insert(String::from("z"));
    args.cell_id_column.get_or_insert(String::from("cell"));
    args.cell_id_unassigned.get_or_insert(String::from("NA"));
    args.scale = 4.0;
}


fn main() {
    // // TODO: Just testing PG sampling
    // {
    //     let mut rng = rand::thread_rng();
    //     // let pg = PolyaGamma::new(1e-6, -80.0);
    //     let mut rs = Vec::<f32>::new();
    //     for _ in 0..100000 {
    //         let pg = PolyaGamma::new(
    //             rng.gen_range(1e-5..1000.0),
    //             rng.gen_range(-50.0..50.0));
    //         rs.push(pg.sample(&mut rng));
    //     }
    //     // dbg!(rs.iter().sum());
    //     // dbg!(pg.mean());
    //     // dbg!(pg.var());
    //     panic!();
    // }

    let mut args = Args::parse();

    if let Some(nthreads) = args.nthreads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(nthreads)
            .build_global()
            .unwrap();
    }
    let nthreads = current_num_threads();
    println!("Using {} threads", nthreads);

    if (args.xenium as u8) + (args.cosmx as u8) + (args.cosmx_micron as u8) + (args.merfish as u8) > 1 {
        panic!("At most one of --xenium, --cosmx, --cosmx-micron, --merfish can be set");
    }

    if args.xenium {
        set_xenium_presets(&mut args);
    }

    if args.cosmx {
        set_cosmx_presets(&mut args);
    }

    if args.cosmx_micron {
        set_cosmx_micron_presets(&mut args);
    }

    if args.merfish {
        set_merfish_presets(&mut args);
    }

    if args.recorded_samples > *args.schedule.last().unwrap() {
        panic!("recorded-samples must be <= the last entry in the schedule");
    }

    assert!(args.ncomponents > 0);

    fn expect_arg<T>(arg: Option<T>, argname: &str) -> T {
        return arg.expect(&format!("Missing required argument: --{}", argname));
    }

    let (transcript_names,
         mut transcripts,
         mut nucleus_assignments,
         mut cell_assignments,
         mut nucleus_population) =
        read_transcripts_csv(
            &args.transcript_csv,
            &expect_arg(args.transcript_column, "transcript-column"),
            args.transcript_id_column,
            args.compartment_column,
            args.compartment_nuclear,
            args.fov_column,
            &expect_arg(args.cell_id_column, "cell-id-column"),
            &expect_arg(args.cell_id_unassigned, "cell-id-unassigned"),
            args.qv_column,
            &expect_arg(args.x_column, "x-column"),
            &expect_arg(args.y_column, "y-column"),
            &expect_arg(args.z_column, "z-column"),
            args.min_qv,
            args.ignore_z_coord,
        );

    // Clamp transcript depth
    // This is we get some reasonable depth slices when we step up to
    // 3d sampling.
    let zs: Vec<f32> = transcripts
        .iter()
        .map(|t| t.z)
        .sorted_by(|a, b| a.partial_cmp(b).unwrap())
        .collect();

    let (q0, q1) = (0.01, 0.99);
    let zmin = zs[(q0 * (zs.len() as f32)) as usize];
    let zmax = zs[(q1 * (zs.len() as f32)) as usize];
    for t in &mut transcripts {
        t.z = t.z.max(zmin).min(zmax);
    }

    let mut ncells = nucleus_population.len();
    let (filtered_transcripts, filtered_nucleus_assignments, filtered_cell_assignments) = filter_cellfree_transcripts(
        &transcripts,
        &nucleus_assignments,
        &cell_assignments,
        ncells,
        args.max_transcript_nucleus_distance,
    );
    transcripts.clone_from(&filtered_transcripts);
    nucleus_assignments.clone_from(&filtered_nucleus_assignments);
    cell_assignments.clone_from(&filtered_cell_assignments);

    // keep removing cells until we can initialize with every cell having at least one voxel
    loop {
        let prev_ncells = ncells;

        filter_sparse_cells(
            args.scale,
            args.voxellayers,
            &transcripts,
            &mut nucleus_assignments,
            &mut cell_assignments,
            &mut nucleus_population,
        );
        ncells = nucleus_population.len();
        if ncells == prev_ncells {
            break;
        }
    }

    let ngenes = transcript_names.len();
    let ncells = nucleus_population.len();
    let ntranscripts = transcripts.len();

    let nucleus_areas = compute_cell_areas(ncells, &transcripts, &nucleus_assignments);
    let mean_nucleus_area = nucleus_areas.iter().sum::<f32>()
        / nucleus_areas.iter().filter(|a| **a > 0.0).count() as f32;

    // If scale isn't specified set it to something reasonable based on mean nuclei size
    let mut scale = args.scale;
    if args.calibrate_scale {
        scale = 0.5 * mean_nucleus_area.sqrt();
    }

    let mut layer_depth = 1.01 * (zmax - zmin) / (args.nlayers as f32);
    if layer_depth == 0.0 {
        layer_depth = 1.0;
    }

    println!("Read {} transcripts", ntranscripts);
    println!("     {} cells", ncells);
    println!("     {} genes", ngenes);

    let (xmin, xmax, ymin, ymax, zmin, zmax) = coordinate_span(&transcripts);
    let (xspan, yspan, mut zspan) = (xmax - xmin, ymax - ymin, zmax - zmin);
    if zspan == 0.0 {
        zspan = 1.0;
    }

    let (mut transcript_density, _total_transcript_density) = estimate_transcript_density(
        &transcripts,
        ngenes,
        layer_depth,
        args.density_sigma,
        args.density_binsize,
        args.density_k,
        args.density_eps,
    );

    // TODO: remove this whole density system
    transcript_density.fill(1.0);

    let full_area = estimate_full_area(&transcripts, mean_nucleus_area);
    println!("Estimated full area: {}", full_area);
    let full_volume = full_area * zspan;

    let full_layer_volume = full_volume / (args.nlayers as f32);
    // let full_layer_volume = total_transcript_density;

    println!("Full volume: {}", full_volume);


    // Find a reasonable grid size to use to chunk the data
    let area = (xmax - xmin) * (ymax - ymin);

    let cell_density = ncells as f32 / area;
    let chunk_size = (args.cells_per_chunk as f32 / cell_density).sqrt();

    let nchunks = |chunk_size: f32, xspan: f32, yspan: f32| {
        ((xspan / chunk_size).ceil() as usize) * ((yspan / chunk_size).ceil() as usize)
    };

    println!(
        "Using grid size {}. Chunks: {}",
        chunk_size,
        nchunks(chunk_size, xspan, yspan)
    );

    let min_cell_volume = 1e-6 * mean_nucleus_area * zspan;

    let priors = ModelPriors {
        dispersion: args.dispersion,
        burnin_dispersion: if args.variable_burnin_dispersion { None } else { Some(10.0) },

        min_cell_volume,

        μ_μ_volume: (2.0 * mean_nucleus_area * zspan).ln(),
        σ_μ_volume: 3.0_f32,
        α_σ_volume: 0.1,
        β_σ_volume: 0.1,

        α_θ: 1.0,
        β_θ: 1.0,

        e_r: 1.0,

        e_h: 1.0,
        f_h: 1.0,

        γ: 1.0,

        α_bg: 1.0,
        β_bg: 1.0,

        α_c: 1.0,
        β_c: 1.0,

        perimeter_eta: 5.3,
        perimeter_bound: args.perimeter_bound,

        nuclear_reassignment_log_prob: args.nuclear_reassignment_prob.ln(),
        nuclear_reassignment_1mlog_prob: (1.0 - args.nuclear_reassignment_prob).ln(),

        prior_seg_reassignment_log_prob: args.prior_seg_reassignment_prob.ln(),
        prior_seg_reassignment_1mlog_prob: (1.0 - args.prior_seg_reassignment_prob).ln(),

        use_diffusion_model: !args.no_diffusion,
        σ_diffusion_proposal: args.diffusion_proposal_sigma,
        p_diffusion: args.diffusion_probability,
        σ_diffusion_near: args.diffusion_sigma_near,
        σ_diffusion_far: args.diffusion_sigma_far,

        σ_z_diffusion_proposal: 0.2 * zspan,
        σ_z_diffusion: 0.2 * zspan,

        zmin,
        zmax,
    };

    let mut params = ModelParams::new(
        &priors,
        full_layer_volume,
        zmin,
        layer_depth,
        &transcripts,
        &transcript_density,
        &nucleus_assignments,
        &nucleus_population,
        &cell_assignments,
        args.ncomponents,
        args.nlayers,
        ncells,
        ngenes,
    );

    let total_iterations = args.schedule.iter().sum::<usize>();
    let mut prog = ProgressBar::new(total_iterations as u64);
    prog.set_style(
        ProgressStyle::with_template("{eta_precise} {bar:60} | {msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    let mut uncertainty = UncertaintyTracker::new();

    let mut sampler = RefCell::new(CubeBinSampler::new(
        &priors,
        &mut params,
        &transcripts,
        transcript_density,
        ngenes,
        args.voxellayers,
        args.nlayers,
        zmin,
        layer_depth,
        scale,
        chunk_size,
    ));
    sampler.borrow_mut().initialize(&priors, &mut params);

    let mut total_steps = 0;

    if args.schedule.len() > 1 {
        run_hexbin_sampler(
            &mut prog,
            sampler.get_mut(),
            &priors,
            &mut params,
            &transcripts,
            args.schedule[0],
            args.local_steps_per_iter,
            None,
            &mut total_steps,
            &args.monitor_cell_polygons,
            args.monitor_cell_polygons_freq,
            true,
            true,
        );

        for &niter in args.schedule[1..args.schedule.len() - 1].iter() {
            if args.check_consistency {
                sampler.borrow_mut().check_consistency(&priors, &mut params);
            }

            sampler.replace_with(|sampler| sampler.double_resolution(&params));
            run_hexbin_sampler(
                &mut prog,
                sampler.get_mut(),
                &priors,
                &mut params,
                &transcripts,
                niter,
                args.local_steps_per_iter,
                None,
                &mut total_steps,
                &args.monitor_cell_polygons,
                args.monitor_cell_polygons_freq,
                true,
                true,
            );
        }
        if args.check_consistency {
            sampler.borrow_mut().check_consistency(&priors, &mut params);
        }
        sampler.replace_with(|sampler| sampler.double_resolution(&params));
    }

    run_hexbin_sampler(
        &mut prog,
        sampler.get_mut(),
        &priors,
        &mut params,
        &transcripts,
        *args.schedule.last().unwrap() - args.recorded_samples,
        args.local_steps_per_iter,
        None,
        &mut total_steps,
        &args.monitor_cell_polygons,
        args.monitor_cell_polygons_freq,
        true,
        false,
    );

    run_hexbin_sampler(
        &mut prog,
        sampler.get_mut(),
        &priors,
        &mut params,
        &transcripts,
        args.recorded_samples,
        args.local_steps_per_iter,
        Some(&mut uncertainty),
        &mut total_steps,
        &args.monitor_cell_polygons,
        args.monitor_cell_polygons_freq,
        true,
        false,
    );

    if args.check_consistency {
        sampler.borrow_mut().check_consistency(&priors, &mut params);
    }
    prog.finish();

    uncertainty.finish(&params);
    let (counts, cell_assignments) = uncertainty.max_posterior_transcript_counts_assignments(
        &params,
        &transcripts,
        args.count_pr_cutoff,
        args.foreground_pr_cutoff,
    );

    let assigned_count = cell_assignments
        .iter()
        .filter(|(c, _)| *c != BACKGROUND_CELL)
        .count();

    let ecounts = uncertainty.expected_counts(&params, &transcripts);
    let cell_centroids = estimate_cell_centroids(&transcripts, &params.cell_assignments, ncells);

    write_expected_counts(
        &args.output_expected_counts,
        &args.output_expected_counts_fmt,
        &transcript_names,
        &ecounts,
    );
    write_counts(
        &args.output_counts,
        &args.output_counts_fmt,
        &transcript_names,
        &counts,
    );
    write_rates(
        &args.output_rates,
        &args.output_rates_fmt,
        &params,
        &transcript_names,
    );
    write_component_params(
        &args.output_component_params,
        &args.output_component_params_fmt,
        &params,
        &transcript_names,
    );
    write_cell_metadata(
        &args.output_cell_metadata,
        &args.output_cell_metadata_fmt,
        &params,
        &cell_centroids,
    );
    write_transcript_metadata(
        &args.output_transcript_metadata,
        &args.output_transcript_metadata_fmt,
        &transcripts,
        &params.transcript_positions,
        &transcript_names,
        &cell_assignments,
        &params.isbackground,
        &params.isconfusion,
    );
    write_gene_metadata(
        &args.output_gene_metadata,
        &args.output_gene_metadata_fmt,
        &params,
        &transcript_names,
        &ecounts,
    );
    write_cubes(
        &args.output_cell_cubes,
        &args.output_cell_cubes_fmt,
        &sampler.borrow(),
    );
    write_cell_multipolygons(&args.output_cell_polygons, &sampler.borrow());
    write_cell_layered_multipolygons(&args.output_cell_polygon_layers, &sampler.borrow());

    if let Some(output_cell_hulls) = args.output_cell_hulls {
        params.write_cell_hulls(&transcripts, &counts, &output_cell_hulls);
    }
}

fn run_hexbin_sampler(
    prog: &mut ProgressBar,
    sampler: &mut CubeBinSampler,
    priors: &ModelPriors,
    params: &mut ModelParams,
    transcripts: &Vec<Transcript>,
    niter: usize,
    local_steps_per_iter: usize,
    mut uncertainty: Option<&mut UncertaintyTracker>,
    total_steps: &mut usize,
    monitor_cell_polygons: &Option<String>,
    monitor_cell_polygons_freq: usize,
    sample_cell_regions: bool,
    burnin: bool,
) {
    sampler.sample_global_params(priors, params, transcripts, &mut uncertainty, burnin);
    let mut proposal_stats = ProposalStats::new();

    for _ in 0..niter {
        // sampler.check_perimeter_bounds(priors);

        if sample_cell_regions {
            // let t0 = std::time::Instant::now();
            for _ in 0..local_steps_per_iter {
                sampler.sample_cell_regions(
                    priors,
                    params,
                    &mut proposal_stats,
                    transcripts,
                    &mut uncertainty,
                );
            }
            // println!("Sample cell regions: {:?}", t0.elapsed());
        }
        // let t0 = std::time::Instant::now();
        sampler.sample_global_params(priors, params, transcripts, &mut uncertainty, burnin);
        // println!("Sample parameters: {:?}", t0.elapsed());

        let nassigned = params.nassigned();
        let nforeground = params.nforeground();
        prog.inc(1);
        prog.set_message(format!(
            "log-likelihood: {ll} | assigned: {nassigned} / {n} ({perc_assigned:.2}%) | non-background: ({perc_foreground:.2}%)",
            ll = params.log_likelihood(priors),
            nassigned = nassigned,
            n = transcripts.len(),
            perc_assigned = 100.0 * (nassigned as f32) / (transcripts.len() as f32),
            perc_foreground = 100.0 * (nforeground as f32) / (transcripts.len() as f32),
        ));

        // println!("Log likelihood: {}", params.log_likelihood());

        // let empty_cell_count = params.cell_population.iter().filter(|p| **p == 0).count();
        // println!("Empty cells: {}", empty_cell_count);

        // dbg!(&proposal_stats);
        proposal_stats.reset();

        // if i % 100 == 0 {
        //     println!("Iteration {} ({} unassigned transcripts)", i, params.nunassigned());
        // }

        if *total_steps % monitor_cell_polygons_freq == 0 {
            if let Some(basename) = monitor_cell_polygons {
                let filename = format!("{}-{:04}.geojson.gz", basename, *total_steps);
                write_cell_layered_multipolygons(&Some(filename), sampler);
            }
        }

        *total_steps += 1;
    }
}
