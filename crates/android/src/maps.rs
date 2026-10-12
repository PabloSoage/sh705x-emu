//! The maps, as the app reads them: a list, one map's numbers, and what is known about it
//! in words. Text is tab- and line-separated; numbers are one flat array.

use crate::{Live, lock};
use session::maps::MapTrace;
use session::sites::Source;

/// Rows a list gives at most.
const ROWS: usize = 1000;
/// Lookups a map's trail gives at most.
const TRAIL: usize = 64;
/// Numbers before the axes in [`Live::map_data`].
pub const MAP_HEADER: usize = 8;

impl Live {
    /// Runs `f` on the maps being traced, if they are.
    fn with_maps<T>(&self, f: impl FnOnce(&MapTrace) -> T) -> Option<T> {
        let slot = lock(&self.maps).clone()?;
        let t = lock(&slot);
        Some(f(&t))
    }

    /// How the trace stands, in one line; empty while the maps are not traced.
    pub fn maps_status(&self) -> String {
        self.with_maps(|t| {
            let m = t.masters;
            let hex = |a: Option<u32>| a.map_or("not found".into(), |a| format!("{a:04X}"));
            format!(
                "{} maps read · {} lookups · routines 3D {}, 2D {} · names: {} variables, {} maps",
                t.seen.len(),
                t.total,
                hex(m.d3),
                hex(m.d2),
                t.names.vars.len(),
                t.names.maps.len()
            )
        })
        .unwrap_or_default()
    }

    /// The maps read, the latest first, those `filter` matches (part of the descriptor in
    /// hex, of the name, or 2D / 3D): one per line, `desc dims nx ny calls last name`,
    /// tab-separated.
    pub fn map_list(&self, filter: &str) -> String {
        let f = filter.trim().to_uppercase();
        self.with_maps(|t| {
            let mut rows: Vec<_> = t
                .seen
                .iter()
                .filter_map(|(&d, s)| {
                    let map = s.map.as_ref()?;
                    let dims = if map.dims() == 3 { "3D" } else { "2D" };
                    let title = t.title(d).unwrap_or_default();
                    let hit = f.is_empty()
                        || format!("{d:06X}").contains(&f)
                        || dims == f
                        || title.to_uppercase().contains(&f);
                    hit.then(|| {
                        (
                            s.at,
                            format!(
                                "{d:06X}\t{}\t{}\t{}\t{}\t{}\t{title}",
                                map.dims(),
                                map.nx(),
                                map.ny(),
                                s.calls,
                                s.last.2
                            ),
                        )
                    })
                })
                .collect();
            rows.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
            rows.into_iter()
                .take(ROWS)
                .map(|(_, r)| r)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
    }

    /// One map's numbers: [`MAP_HEADER`] of them (dims, nx, ny, the last X, Y and result,
    /// lookups, points in the trail), then the X axis, the Y axis (none for a curve), the
    /// cells row by row (`z[j * nx + i]`), and the trail (X, Y, result per lookup, oldest
    /// first). Empty if the map is not known.
    pub fn map_data(&self, desc: u32) -> Vec<f64> {
        self.with_maps(|t| {
            let Some(s) = t.seen.get(&desc) else {
                return Vec::new();
            };
            let Some(map) = &s.map else {
                return Vec::new();
            };
            let trail: Vec<_> = t
                .recent
                .iter()
                .rev()
                .filter(|c| c.r4 == desc)
                .take(TRAIL)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let mut out = Vec::with_capacity(
                MAP_HEADER + map.x.len() + map.y.len() + map.z.len() + 3 * trail.len(),
            );
            out.extend([
                map.dims() as f64,
                map.nx() as f64,
                if map.dims() == 3 {
                    map.ny() as f64
                } else {
                    0.0
                },
                s.last.0 as f64,
                s.last.1 as f64,
                s.last.2 as f64,
                s.calls as f64,
                trail.len() as f64,
            ]);
            out.extend(map.x.iter().map(|&v| v as f64));
            out.extend(map.y.iter().map(|&v| v as f64));
            out.extend(map.z.iter().map(|&v| v as f64));
            for c in trail {
                out.extend([c.fr4 as f64, c.fr5 as f64, c.fr0 as f64]);
            }
            out
        })
        .unwrap_or_default()
    }

    /// What is known about a map, in words, one fact per line: its name, how its cells
    /// are stored, each call site with where its inputs come from and where its result
    /// goes, and which maps read its result.
    pub fn map_text(&self, desc: u32) -> String {
        self.with_maps(|t| {
            let Some(s) = t.seen.get(&desc) else {
                return String::new();
            };
            let Some(map) = &s.map else {
                return format!("{desc:06X}: not a map");
            };
            let mut lines = Vec::new();
            if let Some(title) = t.title(desc) {
                lines.push(title);
            }
            lines.push(if map.cell == session::maps::Cell::Float {
                format!("cells float at {:06X}", map.z_at)
            } else {
                format!(
                    "cells {} × {} + {} at {:06X}",
                    map.cell.name(),
                    map.scale,
                    map.offset,
                    map.z_at
                )
            });
            for (&ret, &n) in &s.sites {
                let Some(site) = t.sites.get(&ret) else {
                    continue;
                };
                let mut l = format!("from {:06X} ({n}×): X ← {}", ret - 4, t.source_text(site.x));
                if map.dims() == 3 {
                    l.push_str(&format!("; Y ← {}", t.source_text(site.y)));
                }
                l.push_str(&match site.out {
                    Some(a) => format!("; result → {}", t.var_name(a)),
                    None => "; result used in the caller".into(),
                });
                lines.push(l);
            }
            let readers = t.readers(desc);
            if !readers.is_empty() {
                let r: Vec<String> = readers.iter().map(|d| format!("{d:06X}")).collect();
                lines.push(format!("read by: {}", r.join(", ")));
            }
            lines.join("\n")
        })
        .unwrap_or_default()
    }

    /// The maps another map's inputs come from (for links), one descriptor per line.
    pub fn map_sources(&self, desc: u32) -> String {
        self.with_maps(|t| {
            let Some(s) = t.seen.get(&desc) else {
                return String::new();
            };
            let mut out: Vec<u32> = s
                .sites
                .keys()
                .filter_map(|r| t.sites.get(r))
                .flat_map(|site| [site.x, site.y])
                .filter_map(|src| match src {
                    Source::Map(d) => Some(d),
                    _ => None,
                })
                .chain(t.readers(desc))
                .collect();
            out.sort_unstable();
            out.dedup();
            out.iter()
                .map(|d| format!("{d:06X}"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
    }
}
