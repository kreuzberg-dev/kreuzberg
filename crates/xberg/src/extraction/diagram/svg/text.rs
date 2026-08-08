//! Label recovery from the source XML.
//!
//! `usvg` is built here without its `text` feature, which wants a font
//! database, and it drops text elements during conversion. This pass
//! reproduces only what that loss cost us, the transform chain down to each
//! `<text>` anchor, so a label lands where the shape it names actually is.

use quick_xml::events::Event;
use usvg::tiny_skia_path::Transform;

use super::super::Label;
use crate::utils::xml_utils::EntityReader;

/// Elements whose text is never drawn on the canvas.
const NON_RENDERING: &[&str] = &["defs", "clipPath", "mask", "marker", "pattern", "symbol", "metadata"];

/// Reads `<text>` anchors and the document title out of the source.
///
/// The transform stack here mirrors what `usvg` applied to the shapes: the
/// viewBox-to-viewport transform at the root, then every `transform` attribute
/// on the way down. Without it a diagram drawn inside a translated `<g>` would
/// have its labels land nowhere near its boxes.
#[derive(Default)]
pub(super) struct TextPass {
    pub(super) title: Option<String>,
    pub(super) labels: Vec<Label>,
    /// Accumulated transform per open element, root transform at the bottom.
    transforms: Vec<Transform>,
    /// Depth of each open element, used only for its length.
    depth: Vec<String>,
    /// Depth at which a non-rendering subtree started, while one is open.
    skipping: Option<usize>,
    /// The label being built, if a `<text>` is open.
    pending: Option<Label>,
    in_title: bool,
}

impl TextPass {
    fn transform(&self) -> Transform {
        self.transforms.last().copied().unwrap_or_else(Transform::identity)
    }

    /// Handle an opening tag, returning the transform its children inherit.
    fn open(&mut self, e: &quick_xml::events::BytesStart<'_>, name: &str) -> Transform {
        let local = attribute(e, "transform")
            .and_then(|v| parse_transform(&v))
            .unwrap_or_else(Transform::identity);
        let combined = self.transform().pre_concat(local);

        if self.skipping.is_none() && NON_RENDERING.contains(&name) {
            self.skipping = Some(self.depth.len());
            return combined;
        }
        if self.skipping.is_some() {
            return combined;
        }

        match name {
            // The document title, or the accessible name of the outermost
            // group, which is where Graphviz and Mermaid put the diagram's
            // name. Deeper than that a `<title>` is a tooltip on one shape.
            "title" if self.depth.len() <= 2 && self.title.is_none() => self.in_title = true,
            "text" | "tspan" => {
                let position = (
                    attribute(e, "x").and_then(|v| first_number(&v)),
                    attribute(e, "y").and_then(|v| first_number(&v)),
                );
                if let (Some(x), Some(y)) = position {
                    // A repositioned `<tspan>` starts a new run: an org chart
                    // draws a name and a job title as two anchors, and joining
                    // them into one string would lose the line break.
                    self.flush();
                    let (x, y) = map_point(&combined, x, y);
                    self.pending = Some(Label {
                        x,
                        y,
                        text: String::new(),
                    });
                } else if name == "text" && self.pending.is_none() {
                    let (x, y) = map_point(&combined, 0.0, 0.0);
                    self.pending = Some(Label {
                        x,
                        y,
                        text: String::new(),
                    });
                }
            }
            _ => {}
        }
        combined
    }

    /// Handle a closing tag for an element opened at `depth`.
    fn close(&mut self, name: &str, depth: usize) {
        if self.skipping == Some(depth) {
            self.skipping = None;
        }
        match name {
            "title" => self.in_title = false,
            "text" => self.flush(),
            _ => {}
        }
    }

    fn push_text(&mut self, raw: &str) {
        let trimmed = raw.trim();
        if self.skipping.is_some() || trimmed.is_empty() {
            return;
        }
        if self.in_title {
            self.title.get_or_insert_with(|| trimmed.to_string());
        } else if let Some(label) = self.pending.as_mut() {
            if !label.text.is_empty() {
                label.text.push(' ');
            }
            label.text.push_str(trimmed);
        }
    }

    fn flush(&mut self) {
        if let Some(label) = self.pending.take()
            && !label.text.is_empty()
        {
            self.labels.push(label);
        }
    }

    pub(super) fn run(mut self, source: &str, canvas: (f32, f32)) -> Self {
        self.transforms.push(root_transform(source, canvas));

        let mut reader = EntityReader::from_str(source);
        reader.config_mut().check_end_names = false;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let name = local_name(e.name().as_ref());
                    let combined = self.open(&e, &name);
                    self.transforms.push(combined);
                    self.depth.push(name);
                }
                // A self-closing element opens and closes in one event, and
                // never passes its transform to a child.
                Ok(Event::Empty(e)) => {
                    let name = local_name(e.name().as_ref());
                    self.open(&e, &name);
                    self.close(&name, self.depth.len());
                }
                Ok(Event::End(_)) => {
                    let name = self.depth.pop().unwrap_or_default();
                    self.transforms.pop();
                    self.close(&name, self.depth.len());
                }
                Ok(Event::Text(e)) => {
                    let raw = String::from_utf8_lossy(e.as_ref());
                    self.push_text(&raw);
                }
                // A malformed tail costs the labels after it and nothing else;
                // the shapes come from a parse `usvg` already accepted.
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }

        self.flush();
        self
    }
}

fn map_point(transform: &Transform, x: f32, y: f32) -> (f32, f32) {
    let mut point = usvg::tiny_skia_path::Point::from_xy(x, y);
    transform.map_point(&mut point);
    (point.x, point.y)
}

/// Strip any namespace prefix: `svg:text` and `text` are the same element.
fn local_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    match name.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => name.into_owned(),
    }
}

fn attribute(e: &quick_xml::events::BytesStart<'_>, wanted: &str) -> Option<String> {
    e.attributes().flatten().find_map(|attr| {
        (local_name(attr.key.as_ref()) == wanted).then(|| String::from_utf8_lossy(&attr.value).trim().to_string())
    })
}

/// First number of an SVG attribute that may hold a list, e.g. `x="10 20 30"`
/// on a `<text>` that positions each glyph.
fn first_number(value: &str) -> Option<f32> {
    let token = value.split([' ', ',', '\t', '\n', '\r']).find(|t| !t.is_empty())?;
    let end = token
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'))
        .unwrap_or(token.len());
    token[..end].parse().ok().filter(|v: &f32| v.is_finite())
}

/// The viewBox-to-viewport transform `usvg` puts at the root of the tree.
///
/// Only the default `xMidYMid meet` and the explicit `none` are handled; the
/// remaining alignments are vanishingly rare in diagrams, and getting one of
/// them wrong shifts labels rather than corrupting the graph.
fn root_transform(source: &str, canvas: (f32, f32)) -> Transform {
    let Some(view_box) = root_attribute(source, "viewBox") else {
        return Transform::identity();
    };
    let numbers: Vec<f32> = view_box
        .split([' ', ',', '\t', '\n', '\r'])
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse::<f32>().ok())
        .collect();
    let [min_x, min_y, width, height] = numbers[..] else {
        return Transform::identity();
    };
    if !(width > 0.0 && height > 0.0) {
        return Transform::identity();
    }

    let (canvas_w, canvas_h) = canvas;
    let (scale_x, scale_y) = (canvas_w / width, canvas_h / height);
    let uniform = !root_attribute(source, "preserveAspectRatio").is_some_and(|v| v.trim().starts_with("none"));

    if uniform {
        let scale = scale_x.min(scale_y);
        Transform::from_translate(
            -min_x * scale + (canvas_w - width * scale) / 2.0,
            -min_y * scale + (canvas_h - height * scale) / 2.0,
        )
        .pre_scale(scale, scale)
    } else {
        Transform::from_translate(-min_x * scale_x, -min_y * scale_y).pre_scale(scale_x, scale_y)
    }
}

/// Read an attribute off the root `<svg>` element without a second full parse.
fn root_attribute(source: &str, wanted: &str) -> Option<String> {
    let start = source.find("<svg")?;
    let end = source[start..].find('>')? + start;
    let tag = &source[start..end];
    let key = format!("{wanted}=");
    let at = tag.find(&key)? + key.len();
    let rest = tag[at..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    rest[1..].split(quote).next().map(str::to_string)
}

/// Parse an SVG `transform` attribute into a single matrix.
fn parse_transform(value: &str) -> Option<Transform> {
    let mut result = Transform::identity();
    let mut rest = value.trim();
    let mut any = false;

    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let close = rest[open..].find(')')? + open;
        let args: Vec<f32> = rest[open + 1..close]
            .split([' ', ',', '\t', '\n', '\r'])
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse::<f32>().ok())
            .collect();
        rest = &rest[close + 1..];

        let step = match (name, args.as_slice()) {
            ("translate", [tx]) => Transform::from_translate(*tx, 0.0),
            ("translate", [tx, ty, ..]) => Transform::from_translate(*tx, *ty),
            ("scale", [s]) => Transform::from_scale(*s, *s),
            ("scale", [sx, sy, ..]) => Transform::from_scale(*sx, *sy),
            ("rotate", [angle]) => Transform::from_rotate(*angle),
            ("rotate", [angle, cx, cy, ..]) => Transform::from_rotate_at(*angle, *cx, *cy),
            ("skewX", [angle]) => Transform::from_row(1.0, 0.0, angle.to_radians().tan(), 1.0, 0.0, 0.0),
            ("skewY", [angle]) => Transform::from_row(1.0, angle.to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
            ("matrix", [a, b, c, d, e, f]) => Transform::from_row(*a, *b, *c, *d, *e, *f),
            _ => continue,
        };
        result = result.pre_concat(step);
        any = true;
    }

    any.then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_attribute_parses_each_primitive() {
        let translate = parse_transform("translate(10, 20)").expect("translate");
        assert_eq!(map_point(&translate, 1.0, 1.0), (11.0, 21.0));

        let scale = parse_transform("scale(2)").expect("scale");
        assert_eq!(map_point(&scale, 3.0, 4.0), (6.0, 8.0));

        let chained = parse_transform("translate(10,0) scale(2)").expect("chained");
        assert_eq!(map_point(&chained, 5.0, 0.0), (20.0, 0.0));

        let matrix = parse_transform("matrix(1 0 0 1 5 5)").expect("matrix");
        assert_eq!(map_point(&matrix, 0.0, 0.0), (5.0, 5.0));

        assert!(parse_transform("").is_none());
        assert!(parse_transform("nonsense").is_none());
    }

    #[test]
    fn attribute_lists_take_their_first_value() {
        assert_eq!(first_number("10 20 30"), Some(10.0));
        assert_eq!(first_number("  -4.5,2"), Some(-4.5));
        assert_eq!(first_number("12px"), Some(12.0));
        assert_eq!(first_number(""), None);
    }
}
