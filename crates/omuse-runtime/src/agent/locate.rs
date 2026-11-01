//! `browser_locate`'s geometry, in code: the choices a vision decision is offered (a coarse grid's cell labels, a
//! zoomed grid's cell numbers, each with `NONE`) and the viewport point a choice stands for. The vision model only
//! picks one of the listed choices as JSON (`Runtime::decide_on_picture`); code computes every coordinate.

/// The column letters of the coarse grid (at most 12 columns).
const LETTERS: &str = "ABCDEFGHIJKL";

/// The choice meaning "not in this picture".
pub const NONE: &str = "NONE";

/// A grid drawn over the viewport (or over a region of it): its size in pixels, its origin and its cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub cols: i64,
    pub rows: i64,
}

impl Grid {
    fn cell_size(&self) -> (f64, f64) {
        (self.w / self.cols as f64, self.h / self.rows as f64)
    }

    /// The coarse grid's labels, row by row ("A1" … "H6"), then `NONE`.
    pub fn labels(&self) -> Vec<String> {
        let cols = self.cols.clamp(1, LETTERS.len() as i64) as usize;
        let mut out: Vec<String> = (1..=self.rows.max(1))
            .flat_map(|r| LETTERS[..cols].chars().map(move |c| format!("{c}{r}")))
            .collect();
        out.push(NONE.into());
        out
    }

    /// The zoomed grid's numbers ("1" … cols×rows, left to right, then top to bottom), then `NONE`.
    pub fn numbers(&self) -> Vec<String> {
        let mut out: Vec<String> = (1..=(self.cols * self.rows).max(1))
            .map(|n| n.to_string())
            .collect();
        out.push(NONE.into());
        out
    }

    /// The (column, row) of a listed label ("C4" → (2, 3)); `None` for `NONE` or anything off the grid.
    fn cell_of_label(&self, label: &str) -> Option<(i64, i64)> {
        let mut ch = label.chars();
        let c = LETTERS.find(ch.next()?)? as i64;
        let r = ch.as_str().parse::<i64>().ok()? - 1;
        (c < self.cols && (0..self.rows).contains(&r)).then_some((c, r))
    }

    /// The centre of a labelled cell, and the region to zoom into: that cell plus half a cell around it.
    pub fn label_point(&self, label: &str) -> Option<((f64, f64), [f64; 4])> {
        let (c, r) = self.cell_of_label(label)?;
        let (cw, ch) = self.cell_size();
        let centre = (
            self.x + (c as f64 + 0.5) * cw,
            self.y + (r as f64 + 0.5) * ch,
        );
        let region = [
            (self.x + c as f64 * cw - cw / 2.0).max(0.0),
            (self.y + r as f64 * ch - ch / 2.0).max(0.0),
            cw * 2.0,
            ch * 2.0,
        ];
        Some((centre, region))
    }

    /// The centre of a numbered cell ("17"); `None` for `NONE` or a number off the grid.
    pub fn number_point(&self, number: &str) -> Option<(f64, f64)> {
        let n = number.parse::<i64>().ok()?;
        if !(1..=self.cols * self.rows).contains(&n) {
            return None;
        }
        let k = n - 1;
        let (cw, ch) = self.cell_size();
        Some((
            self.x + ((k % self.cols) as f64 + 0.5) * cw,
            self.y + ((k / self.cols) as f64 + 0.5) * ch,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Grid {
        Grid {
            x: 0.0,
            y: 0.0,
            w: 1280.0,
            h: 800.0,
            cols: 8,
            rows: 6,
        }
    }

    #[test]
    fn the_choices_are_every_cell_and_none() {
        let g = viewport();
        let l = g.labels();
        assert_eq!(l.len(), 49);
        assert_eq!(
            (l[0].as_str(), l[7].as_str(), l[8].as_str()),
            ("A1", "H1", "A2")
        );
        assert_eq!(l.last().map(String::as_str), Some(NONE));
        let n = g.numbers();
        assert_eq!(
            (n[0].as_str(), n[47].as_str(), n[48].as_str()),
            ("1", "48", NONE)
        );
    }

    #[test]
    fn a_chosen_cell_is_its_centre_and_a_zoom_region_around_it() {
        let g = viewport();
        let ((x, y), region) = g.label_point("C4").unwrap();
        let r1 = |v: f64| (v * 10.0).round() / 10.0;
        assert_eq!((r1(x), r1(y)), (400.0, 466.7));
        assert_eq!(region.map(r1), [240.0, 333.3, 320.0, 266.7]);
        // the first cell: the region is clipped at the page's edge
        assert_eq!(g.label_point("A1").unwrap().1[..2], [0.0, 0.0]);
        // NONE and cells off the grid stand for no point
        for bad in [NONE, "I1", "A7", "", "c4x"] {
            assert!(g.label_point(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_zoomed_number_is_its_cell_centre_inside_the_region() {
        let z = Grid {
            x: 240.0,
            y: 320.0,
            w: 320.0,
            h: 240.0,
            cols: 8,
            rows: 6,
        };
        assert_eq!(z.number_point("1"), Some((260.0, 340.0)));
        assert_eq!(z.number_point("10"), Some((300.0, 380.0)));
        assert_eq!(z.number_point("48"), Some((540.0, 540.0)));
        assert!(z.number_point("49").is_none() && z.number_point(NONE).is_none());
    }
}
