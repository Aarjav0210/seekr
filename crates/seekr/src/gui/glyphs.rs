//! Box-drawing and block characters drawn as shapes, the way terminals do it:
//! font glyphs leave gaps between rows and differ from font to font, and the
//! wordmark, the frames and the calculator's big digits are made of them.

/// A filled rectangle in a cell, in points from the cell's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Opacity of the fill (the shade characters ░▒▓).
    pub alpha: f64,
}

/// The shapes that make `ch` in a `w`×`h` cell, or None for characters the
/// font draws. `light` is the thin line's width; heavy lines are twice that.
pub fn shapes(ch: char, w: f64, h: f64, light: f64) -> Option<Vec<Shape>> {
    block(ch, w, h).or_else(|| lines(ch, w, h, light))
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
    Shape { x, y, w, h, alpha: 1.0 }
}

/// U+2580…U+2595: halves, eighths, shades, the full block.
fn block(ch: char, w: f64, h: f64) -> Option<Vec<Shape>> {
    let eighths = |n: f64| n / 8.0;
    let s = match ch {
        '█' => rect(0.0, 0.0, w, h),
        '▀' => rect(0.0, 0.0, w, h / 2.0),
        '▄' => rect(0.0, h / 2.0, w, h / 2.0),
        '▌' => rect(0.0, 0.0, w / 2.0, h),
        '▐' => rect(w / 2.0, 0.0, w / 2.0, h),
        '▔' => rect(0.0, 0.0, w, h * eighths(1.0)),
        '▕' => rect(w * eighths(7.0), 0.0, w * eighths(1.0), h),
        '░' | '▒' | '▓' => {
            let alpha = match ch {
                '░' => 0.25,
                '▒' => 0.5,
                _ => 0.75,
            };
            Shape { alpha, ..rect(0.0, 0.0, w, h) }
        }
        // ▁▂▃▄▅▆▇: lower n eighths.
        '\u{2581}'..='\u{2587}' => {
            let n = (ch as u32 - 0x2580) as f64;
            rect(0.0, h * (1.0 - eighths(n)), w, h * eighths(n))
        }
        // ▉▊▋▌▍▎▏: left 7…1 eighths.
        '\u{2589}'..='\u{258f}' => {
            let n = (0x2590 - ch as u32) as f64;
            rect(0.0, 0.0, w * eighths(n), h)
        }
        _ => return None,
    };
    Some(vec![s])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Weight {
    None,
    Light,
    Heavy,
    Double,
}

/// Up, right, down, left for U+2500…U+257F, one digit each: 0 none, 1 light,
/// 2 heavy, 3 double. Dashed lines are drawn solid and arcs as corners;
/// "" for the diagonals, which the font draws.
const ARMS: [&str; 128] = [
    "0101", "0202", "1010", "2020", "0101", "0202", "1010", "2020", // ─━│┃┄┅┆┇
    "0101", "0202", "1010", "2020", "0110", "0210", "0120", "0220", // ┈┉┊┋┌┍┎┏
    "0011", "0012", "0021", "0022", "1100", "1200", "2100", "2200", // ┐┑┒┓└┕┖┗
    "1001", "1002", "2001", "2002", "1110", "1210", "2110", "1120", // ┘┙┚┛├┝┞┟
    "2120", "2210", "1220", "2220", "1011", "1012", "2011", "1021", // ┠┡┢┣┤┥┦┧
    "2021", "2012", "1022", "2022", "0111", "0112", "0211", "0212", // ┨┩┪┫┬┭┮┯
    "0121", "0122", "0221", "0222", "1101", "1102", "1201", "1202", // ┰┱┲┳┴┵┶┷
    "2101", "2102", "2201", "2202", "1111", "1112", "1211", "1212", // ┸┹┺┻┼┽┾┿
    "2111", "1121", "2121", "2112", "2211", "1122", "1221", "2212", // ╀╁╂╃╄╅╆╇
    "1222", "2122", "2221", "2222", "0101", "0202", "1010", "2020", // ╈╉╊╋╌╍╎╏
    "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033", // ═║╒╓╔╕╖╗
    "1300", "3100", "3300", "1003", "3001", "3003", "1310", "3130", // ╘╙╚╛╜╝╞╟
    "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303", // ╠╡╢╣╤╥╦╧
    "3101", "3303", "1313", "3131", "3333", "0110", "0011", "1001", // ╨╩╪╫╬╭╮╯
    "1100", "", "", "", "0001", "1000", "0100", "0010", //             ╰╱╲╳╴╵╶╷
    "0002", "2000", "0200", "0020", "0201", "1020", "0102", "2010", // ╸╹╺╻╼╽╾╿
];

fn lines(ch: char, w: f64, h: f64, light: f64) -> Option<Vec<Shape>> {
    let i = (ch as u32).checked_sub(0x2500).filter(|i| *i < 128)?;
    let spec = ARMS[i as usize].as_bytes();
    if spec.is_empty() {
        return None;
    }
    let weight = |b: u8| match b {
        b'1' => Weight::Light,
        b'2' => Weight::Heavy,
        b'3' => Weight::Double,
        _ => Weight::None,
    };
    let (up, right, down, left) = (weight(spec[0]), weight(spec[1]), weight(spec[2]), weight(spec[3]));
    let (cx, cy) = (w / 2.0, h / 2.0);
    let thick = |wt: Weight| if wt == Weight::Heavy { light * 2.0 } else { light };
    // The two strokes of a double line sit this far either side of the centre.
    let gap = light * 1.5;
    let mut out = Vec::new();

    // Single arms run from the edge to the centre, overlapping it by half
    // their width so that corners and joins close.
    let single = |out: &mut Vec<Shape>, horizontal: bool, toward_start: bool, wt: Weight| {
        let t = thick(wt);
        out.push(if horizontal {
            if toward_start {
                rect(0.0, cy - t / 2.0, cx + t / 2.0, t)
            } else {
                rect(cx - t / 2.0, cy - t / 2.0, w - cx + t / 2.0, t)
            }
        } else if toward_start {
            rect(cx - t / 2.0, 0.0, t, cy + t / 2.0)
        } else {
            rect(cx - t / 2.0, cy - t / 2.0, t, h - cy + t / 2.0)
        });
    };

    // A double arm's two strokes stop short of the centre (an inner corner)
    // when the double line across it turns their way, run past it (an outer
    // corner) when it turns the other way, and meet the centre otherwise.
    let reach = |toward: Weight, away: Weight| {
        if toward == Weight::Double {
            -gap
        } else if away == Weight::Double {
            gap
        } else {
            light / 2.0
        }
    };

    if left == Weight::Double || right == Weight::Double {
        for s in [-1.0, 1.0] {
            let y = cy + s * gap - light / 2.0;
            // The stroke above the centre (s < 0) turns into the arm going up.
            let (toward, away) = if s < 0.0 { (up, down) } else { (down, up) };
            if left == Weight::Double {
                out.push(rect(0.0, y, cx + reach(toward, away), light));
            }
            if right == Weight::Double {
                let start = cx - reach(toward, away);
                out.push(rect(start, y, w - start, light));
            }
        }
    }
    if up == Weight::Double || down == Weight::Double {
        for s in [-1.0, 1.0] {
            let x = cx + s * gap - light / 2.0;
            let (toward, away) = if s < 0.0 { (left, right) } else { (right, left) };
            if up == Weight::Double {
                out.push(rect(x, 0.0, light, cy + reach(toward, away)));
            }
            if down == Weight::Double {
                let start = cy - reach(toward, away);
                out.push(rect(x, start, light, h - start));
            }
        }
    }
    for (wt, horizontal, toward_start) in [(left, true, true), (right, true, false), (up, false, true), (down, false, false)] {
        if wt == Weight::Light || wt == Weight::Heavy {
            single(&mut out, horizontal, toward_start, wt);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f64 = 8.0;
    const H: f64 = 16.0;

    fn covers(shapes: &[Shape], x: f64, y: f64) -> bool {
        shapes.iter().any(|s| x >= s.x && x < s.x + s.w && y >= s.y && y < s.y + s.h)
    }

    #[test]
    fn the_table_lines_up_with_the_characters() {
        assert_eq!(ARMS[('┏' as u32 - 0x2500) as usize], "0220");
        assert_eq!(ARMS[('╋' as u32 - 0x2500) as usize], "2222");
        assert_eq!(ARMS[('╔' as u32 - 0x2500) as usize], "0330");
        assert_eq!(ARMS[('╬' as u32 - 0x2500) as usize], "3333");
        assert_eq!(ARMS[('╰' as u32 - 0x2500) as usize], "1100");
        assert_eq!(ARMS[('╺' as u32 - 0x2500) as usize], "0200");
        assert_eq!(ARMS[('╿' as u32 - 0x2500) as usize], "2010");
        assert!(shapes('╱', W, H, 1.0).is_none(), "diagonals are the font's");
    }

    #[test]
    fn lines_reach_the_cell_edges_so_rows_join() {
        let v = shapes('│', W, H, 1.0).unwrap();
        assert!(covers(&v, W / 2.0, 0.0) && covers(&v, W / 2.0, H - 0.01));
        let h = shapes('─', W, H, 1.0).unwrap();
        assert!(covers(&h, 0.0, H / 2.0) && covers(&h, W - 0.01, H / 2.0));
        // ┌ goes right and down from the centre, not up or left.
        let c = shapes('┌', W, H, 1.0).unwrap();
        assert!(covers(&c, W - 0.01, H / 2.0) && covers(&c, W / 2.0, H - 0.01));
        assert!(!covers(&c, 0.5, H / 2.0) && !covers(&c, W / 2.0, 0.5));
    }

    #[test]
    fn double_corners_nest() {
        let (cx, cy, g) = (W / 2.0, H / 2.0, 1.5);
        // ╔: the outer stroke turns at the top left, the inner one inside it.
        let s = shapes('╔', W, H, 1.0).unwrap();
        assert!(covers(&s, cx - g, cy - g), "outer corner closed");
        assert!(covers(&s, cx + g, cy + g), "inner corner closed");
        assert!(covers(&s, W - 0.01, cy - g) && covers(&s, W - 0.01, cy + g), "both strokes reach the right edge");
        assert!(covers(&s, cx - g, H - 0.01) && covers(&s, cx + g, H - 0.01), "both strokes reach the bottom");
        assert!(!covers(&s, cx + 2.5, cy) && !covers(&s, cx, cy + 3.0), "nothing between the strokes");
        assert!(!covers(&s, 0.5, cy - g) && !covers(&s, cx - g, 0.5), "nothing up or left");
        // ═ is two full-width strokes, nothing in between.
        let d = shapes('═', W, H, 1.0).unwrap();
        assert!(covers(&d, 0.0, cy - g) && covers(&d, W - 0.01, cy + g));
        assert!(!covers(&d, cx, cy));
    }

    #[test]
    fn blocks() {
        assert_eq!(shapes('█', W, H, 1.0), Some(vec![rect(0.0, 0.0, W, H)]));
        assert_eq!(shapes('▄', W, H, 1.0), Some(vec![rect(0.0, H / 2.0, W, H / 2.0)]));
        assert_eq!(shapes('▂', W, H, 1.0), Some(vec![rect(0.0, H * 0.75, W, H * 0.25)]));
        assert_eq!(shapes('▏', W, H, 1.0), Some(vec![rect(0.0, 0.0, W / 8.0, H)]));
        assert_eq!(shapes('▒', W, H, 1.0).unwrap()[0].alpha, 0.5);
        assert!(shapes('a', W, H, 1.0).is_none());
    }
}
