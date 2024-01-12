
use std::collections::HashSet;
use super::cubebinsampler::{Cube, CubeLayout};


use geo::geometry::{LineString, MultiPolygon, Polygon};
use geo::algorithm::simplify::Simplify;
use petgraph::adj;
use itertools::Itertools;


// TODO: Plan for polygon generation:
//   - For each cell (in parallel)
//   -   for each voxel in the cells
//   -     construct a vec of all mismatching edges (end coordinates)
//   -     traverse these edges to get outlines
//   -     these outlines are the polygons
//
// Edge simplification:
//   To reduce the jagged edges: 

pub struct PolygonBuilder {
    edges: Vec<(i32, (i32, i32), (i32, i32))>,
    visited: Vec<bool>,
}


fn reverse_edge(edge: &(i32, (i32, i32), (i32, i32))) -> (i32, (i32, i32), (i32, i32)) {
    return (edge.0, edge.2, edge.1);
}


// return the lexigraphically next point
fn next_point(v: (i32, i32)) -> (i32, i32) {
    return (v.0, v.1+1);
}


fn mark_visited(edges_k: &[(i32, (i32, i32), (i32, i32))], visited_k: &mut [bool], edge: &(i32, (i32, i32), (i32, i32))) {
    let pos = edges_k.binary_search(edge).unwrap();
    assert!(!visited_k[pos]);
    visited_k[pos] = true;

    let pos = edges_k.binary_search(&reverse_edge(edge)).unwrap();
    assert!(!visited_k[pos]);
    visited_k[pos] = true;
}


fn antialias_polygon(polygon: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    let mut smoothed_polygon = Vec::new();
    if polygon.len() <= 5 {
        return polygon.clone();
    }

    smoothed_polygon.push(polygon[0]);
    smoothed_polygon.push(polygon[1]);

    for (p1, u, v, w, p2) in polygon.iter().tuple_windows::<(_,_,_,_,_)>() {
        // dbg!(p1, u, v, w, p2);

        let δi_v = v.0 - u.0;
        let δj_v = v.1 - u.1;
        let δi_w = w.0 - v.0;
        let δj_w = w.1 - v.1;

        if ((δi_v == 0) != (δi_w == 0)) && ((δj_v == 0) != (δj_w == 0)) {
            // (u, v, w) forms a stair step. Smooth the polygon by skipping over
            // vertex v on some conditions.

            if ((w.0 - p1.0).abs() + (w.1 - p1.1).abs()) != 3 || ((u.0 - p2.0).abs() + (u.1 - p2.1).abs()) != 3 {
                smoothed_polygon.push(*v);
            }
        } else {
            smoothed_polygon.push(*v);
        }
    }

    smoothed_polygon.push(polygon[polygon.len()-2]);
    smoothed_polygon.push(polygon.last().unwrap().clone());

    assert!(smoothed_polygon.first() == smoothed_polygon.last());

    return smoothed_polygon;
}


// This is an exact simplification algorithm: we just want to merge segments
// that are part of the same line.
fn simplify_polygon(polygon: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
    if polygon.len() <= 3 {
        return polygon.clone();
    }

    let mut simplified_polygon = Vec::new();
    simplified_polygon.push(*polygon.first().unwrap());

    for (u, v, w) in polygon.iter().tuple_windows::<(_,_,_)>() {
        // If v is colinear with (u, w), then skip it, otherwise push it.
        let δi_v = v.0 - u.0;
        let δj_v = v.1 - u.1;
        let δi_w = w.0 - v.0;
        let δj_w = w.1 - v.1;

        if δi_v == δi_w && δj_v == δj_w {
            continue;
        } else {
            simplified_polygon.push(*v);
        }
    }
    simplified_polygon.push(simplified_polygon.first().unwrap().clone());

    return simplified_polygon;
}


impl PolygonBuilder {
    pub fn new() -> Self {
        return PolygonBuilder {
            edges: Vec::new(),
            visited: Vec::new(),
        };
    }

    pub fn cell_voxels_to_polygons(&mut self, layout: &CubeLayout, voxels: &HashSet<Cube>) -> Vec<(i32, MultiPolygon<f32>)> {
        // if we store edges in in μm, we run the risk of failing line up points
        // due to numerical imprecision. Instead we use integer coordinates,
        // where (i, j, k) represents the corner of voxel (i, j, k)

        // construct a data structure containing every cell edge (encoded in voxel coordinates)
        self.edges.clear();
        let mut kmin = i32::MAX;
        let mut kmax = i32::MIN;
        for voxel in voxels {
            kmin = kmin.min(voxel.k);
            kmax = kmax.max(voxel.k);

            for neighbor in voxel.von_neumann_neighborhood_xy() {
                if !voxels.contains(&neighbor) {
                    let edge = voxel.edge_xy(&neighbor);
                    self.edges.push((voxel.k, edge.0, edge.1));
                    self.edges.push((voxel.k, edge.1, edge.0));
                }
            }
        }
        self.edges.sort_unstable();

        // traverse the cell edges to construct polygons
        self.visited.fill(false);
        self.visited.resize(self.edges.len(), false);

        let mut multipolygons = Vec::new();

        for k in kmin..kmax+1 {
            let first = self.edges.partition_point(|edge| edge.0 < k);
            let last = self.edges.partition_point(|edge| edge.0 < k+1);

            let edges_k = &self.edges[first..last];
            let visited_k = &mut self.visited[first..last];

            let nedges = edges_k.len();
            let mut nvisited = 0;

            let mut polygons_k = Vec::new();

            while nvisited < nedges {
                let mut polygon = Vec::new();

                let pos = visited_k.iter().position(|v| !v);

                if let Some(pos) = pos {
                    let edge = edges_k[pos];
                    mark_visited(edges_k, visited_k, &edge);
                    nvisited += 2;

                    polygon.push(edge.1);
                    polygon.push(edge.2);

                    let mut u = edge.1;
                    let mut v = edge.2;

                    while nvisited < nedges {
                        let δi = v.0 - u.0;
                        let δj = v.1 - u.1;
                        assert!(δi.abs() + δj.abs() == 1);

                        let first = edges_k.partition_point(|edge| edge.1 < v);
                        let last = edges_k.partition_point(|edge| edge.1 < next_point(v));
                        let adjacent_edges = &edges_k[first..last];
                        let adjacent_edges_visited = &mut visited_k[first..last];

                        // we have either an unambiguous path or we are at the corner of two voxels
                        assert!(adjacent_edges.len() == 2 || adjacent_edges.len() == 4);

                        if adjacent_edges.len() == 2 {
                            let adjacent_edge;
                            if *adjacent_edges.first().unwrap() == (k, v, u) {
                                adjacent_edge = adjacent_edges.last().unwrap();
                                if *adjacent_edges_visited.last().unwrap() {
                                    assert!(polygon.first() == polygon.last());
                                    break;
                                }
                            } else {
                                adjacent_edge = adjacent_edges.first().unwrap();
                                if *adjacent_edges_visited.first().unwrap() {
                                    assert!(polygon.first() == polygon.last());
                                    break;
                                }
                            }

                            mark_visited(edges_k, visited_k, &adjacent_edge);
                            nvisited += 2;
                            polygon.push(adjacent_edge.2);

                            u = v;
                            v = adjacent_edge.2;
                        } else {
                            let w;

                            // Might be a nicer way, but I'm just going to handle
                            // each case exhaustively here.
                            if voxels.contains(&Cube::new(k, v.0, v.1)) {
                                if δi == -1 {
                                    w = (v.0, v.1-1);
                                } else if δi == 1 {
                                    w = (v.0, v.1+1);
                                } else if δj == -1 {
                                    w = (v.0-1, v.1);
                                } else if δj == 1 {
                                    w = (v.0+1, v.1);
                                } else {
                                    unreachable!();
                                }
                            } else {
                                if δi == -1 {
                                    w = (v.0, v.1+1);
                                } else if δi == 1 {
                                    w = (v.0, v.1-1);
                                } else if δj == -1 {
                                    w = (v.0+1, v.1);
                                } else if δj == 1 {
                                    w = (v.0-1, v.1);
                                } else {
                                    unreachable!();
                                }
                            }
                            let adjacent_edge = (k, v, w);
                            assert!(adjacent_edges.contains(&adjacent_edge));
                            if adjacent_edges_visited[adjacent_edges.iter().position(|e| *e == adjacent_edge).unwrap()] {
                                assert!(polygon.first() == polygon.last());
                                break;
                            }

                            mark_visited(edges_k, visited_k, &adjacent_edge);
                            nvisited += 2;
                            polygon.push(w);
                            u = v;
                            v = w;
                        }
                    }

                    assert!(polygon.first() == polygon.last());

                    let polygon = antialias_polygon(polygon);
                    let polygon = simplify_polygon(polygon);

                    // convert coordinates to μm
                    let polygon: Vec<(f32, f32)> = polygon
                        .iter()
                        .map(|v| {
                            let (x, y, _z) = layout.cube_corner_to_world_pos(Cube::new(v.0, v.1, 0));
                            return (x, y);
                        })
                        .collect();

                    polygons_k.push(Polygon::<f32>::new(
                        LineString::from(polygon),
                        Vec::new()));
                } else {
                    // no unvisited edges left
                    break;
                }
            }

            multipolygons.push((k, MultiPolygon::new(polygons_k)));
        }

        return multipolygons;
    }
}
