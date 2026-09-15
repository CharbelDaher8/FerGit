//! Test-only ASCII renderer for graph rows.
//!
//! Each row is drawn as up to three text lines: its upper half, the node line, and its lower half
//! (half lines with no segments are omitted). Lane `i` sits at character `2 * i`.
//!
//! - `*` is the node; `|` is a straight segment (or a lane passing the node line).
//! - `/` and `\` are diagonals, placed in the odd cell next to the lane where the segment starts;
//!   `_` extends a diagonal that spans more than one lane.
//!
//! Next to the drawing, the node line shows the id and node color (`c3`), and each half line lists
//! its segments as `from>to:color`, or `lane:color` when straight. The list is exact where the
//! drawing is ambiguous (for example several diagonals leaving a node on the same side).

use fergit_graph::{Edge, GraphRow, Half, Layout};

/// Lays out `spec` (one commit per line: `id parent parent...`, children first) and draws it.
pub fn draw(spec: &str) -> String {
    let commits: Vec<(&str, Vec<&str>)> = spec
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((words.next()?, words.collect()))
        })
        .collect();
    let mut layout = Layout::new();
    let rows: Vec<GraphRow> = commits.iter().map(|(id, parents)| layout.push(*id, parents)).collect();
    let ids: Vec<&str> = commits.iter().map(|(id, _)| *id).collect();
    render(&ids, &rows)
}

pub fn render(ids: &[&str], rows: &[GraphRow]) -> String {
    let lanes = rows
        .iter()
        .flat_map(|row| std::iter::once(row.column).chain(row.edges.iter().flat_map(|e| [e.from, e.to])))
        .max()
        .map_or(1, |max| usize::from(max) + 1);
    let width = 2 * lanes - 1;
    let id_width = ids.iter().map(|id| id.len()).max().unwrap_or(0);

    let mut out = String::new();
    let mut line = |drawing: String, text: String| {
        let full = format!("{drawing:<width$}  {text}");
        out.push_str(full.trim_end());
        out.push('\n');
    };

    for (id, row) in ids.iter().zip(rows) {
        let upper: Vec<Edge> = row.edges.iter().copied().filter(|e| e.half == Half::Upper).collect();
        let lower: Vec<Edge> = row.edges.iter().copied().filter(|e| e.half == Half::Lower).collect();

        if !upper.is_empty() {
            line(draw_half(&upper, width), format!("{:id_width$}  {}", "", segments(&upper)));
        }

        let mut node = vec![' '; width];
        for e in &upper {
            if e.from == e.to && e.from != row.column {
                node[2 * usize::from(e.from)] = '|';
            }
        }
        node[2 * usize::from(row.column)] = '*';
        line(node.into_iter().collect(), format!("{id:id_width$}  c{}", row.color));

        if !lower.is_empty() {
            line(draw_half(&lower, width), format!("{:id_width$}  {}", "", segments(&lower)));
        }
    }
    out
}

fn draw_half(edges: &[Edge], width: usize) -> String {
    let mut cells = vec![' '; width];
    // Underscores first, so strokes drawn afterwards win where they overlap.
    for e in edges {
        // A segment starts at `a` (top or center) and ends at `b` (center or bottom). The run lies
        // between the diagonal stroke next to `a` and the lane at `b`.
        let (a, b) = (2 * usize::from(e.from), 2 * usize::from(e.to));
        let run = match a.cmp(&b) {
            std::cmp::Ordering::Equal => continue,
            std::cmp::Ordering::Greater => b + 1..a - 1,
            std::cmp::Ordering::Less => a + 2..b,
        };
        for cell in &mut cells[run] {
            *cell = '_';
        }
    }
    for e in edges {
        let (a, b) = (2 * usize::from(e.from), 2 * usize::from(e.to));
        let (at, stroke) = match a.cmp(&b) {
            std::cmp::Ordering::Equal => (a, '|'),
            std::cmp::Ordering::Greater => (a - 1, '/'),
            std::cmp::Ordering::Less => (a + 1, '\\'),
        };
        cells[at] = stroke;
    }
    cells.into_iter().collect()
}

fn segments(edges: &[Edge]) -> String {
    edges
        .iter()
        .map(|e| {
            if e.from == e.to {
                format!("{}:c{}", e.from, e.color)
            } else {
                format!("{}>{}:c{}", e.from, e.to, e.color)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
