//! Icons as vector shapes in a 10×10 view box, so they don't depend on font coverage.

use rune::{Color, El, LineCap, Shape, canvas};

fn icon(size: f32, s: Shape) -> El {
    canvas(s.view_box(10.0, 10.0)).size(size, size)
}

pub fn play(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new().fill(vec![[2.5, 1.5], [9.0, 5.0], [2.5, 8.5]], c),
    )
}

pub fn pause(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new()
            .fill(vec![[2.0, 1.5], [4.0, 1.5], [4.0, 8.5], [2.0, 8.5]], c)
            .fill(vec![[6.0, 1.5], [8.0, 1.5], [8.0, 8.5], [6.0, 8.5]], c),
    )
}

pub fn down(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new()
            .line([5.0, 1.0], [5.0, 7.0], 1.2, LineCap::Round, c)
            .stroke(vec![[2.2, 4.6], [5.0, 7.4], [7.8, 4.6]], 1.2, c)
            .line([1.8, 9.2], [8.2, 9.2], 1.2, LineCap::Round, c),
    )
}

pub fn transfers(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new()
            .line([3.0, 1.0], [3.0, 9.0], 1.1, LineCap::Round, c)
            .stroke(vec![[1.0, 3.0], [3.0, 1.0], [5.0, 3.0]], 1.1, c)
            .line([7.0, 1.0], [7.0, 9.0], 1.1, LineCap::Round, c)
            .stroke(vec![[5.0, 7.0], [7.0, 9.0], [9.0, 7.0]], 1.1, c),
    )
}

fn ring(cx: f32, cy: f32, r: f32, from: f32, to: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

pub fn gear(size: f32, c: Color) -> El {
    use std::f32::consts::TAU;
    // A toothed outline: 7 teeth alternating outer and inner radius, plus a hub.
    let teeth = 7;
    let mut outline = Vec::new();
    for i in 0..teeth {
        let a0 = i as f32 / teeth as f32 * TAU;
        let step = TAU / teeth as f32;
        for (k, r) in [(0.0, 3.4), (0.18, 4.4), (0.5, 4.4), (0.68, 3.4)] {
            let a = a0 + step * k;
            outline.push([5.0 + r * a.cos(), 5.0 + r * a.sin()]);
        }
    }
    outline.push(outline[0]);
    icon(
        size,
        Shape::new()
            .stroke(outline, 1.0, c)
            .stroke(ring(5.0, 5.0, 1.4, 0.0, TAU, 16), 1.0, c),
    )
}

pub fn speaker(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new()
            .fill(
                vec![
                    [1.0, 3.6],
                    [3.0, 3.6],
                    [5.6, 1.4],
                    [5.6, 8.6],
                    [3.0, 6.4],
                    [1.0, 6.4],
                ],
                c,
            )
            .stroke(vec![[7.0, 3.4], [7.6, 5.0], [7.0, 6.6]], 1.0, c)
            .stroke(vec![[8.2, 2.2], [9.2, 5.0], [8.2, 7.8]], 1.0, c),
    )
}

pub fn back(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new()
            .line([1.5, 5.0], [8.5, 5.0], 1.2, LineCap::Round, c)
            .stroke(vec![[4.5, 2.0], [1.5, 5.0], [4.5, 8.0]], 1.2, c),
    )
}

pub fn close(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new()
            .line([2.0, 2.0], [8.0, 8.0], 1.2, LineCap::Round, c)
            .line([8.0, 2.0], [2.0, 8.0], 1.2, LineCap::Round, c),
    )
}

pub fn check(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new().stroke(vec![[1.8, 5.2], [4.0, 7.4], [8.4, 2.6]], 1.3, c),
    )
}

pub fn retry(size: f32, c: Color) -> El {
    use std::f32::consts::PI;
    icon(
        size,
        Shape::new()
            .stroke(ring(5.0, 5.0, 3.4, -0.35 * PI, 1.45 * PI, 20), 1.2, c)
            .fill(vec![[6.6, 0.4], [9.4, 2.3], [6.2, 3.6]], c),
    )
}

pub fn chevron(size: f32, open: bool, c: Color) -> El {
    let pts = if open {
        vec![[2.5, 3.8], [5.0, 6.3], [7.5, 3.8]]
    } else {
        vec![[3.8, 2.5], [6.3, 5.0], [3.8, 7.5]]
    };
    icon(size, Shape::new().stroke(pts, 1.2, c))
}

pub fn folder(size: f32, c: Color) -> El {
    icon(
        size,
        Shape::new().stroke(
            vec![
                [1.0, 2.2],
                [3.8, 2.2],
                [4.8, 3.4],
                [9.0, 3.4],
                [9.0, 8.2],
                [1.0, 8.2],
                [1.0, 2.2],
            ],
            1.0,
            c,
        ),
    )
}
