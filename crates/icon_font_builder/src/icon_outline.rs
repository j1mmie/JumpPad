//! Turns a Lucide drawing into the filled outline a glyph is made of.
//!
//! Lucide draws with strokes. OpenType has no stroke - a glyph is filled
//! contours and nothing else - so the pen width is baked in here and becomes
//! a property of the font from that point on. Nothing a draw call can pass
//! will make the icons heavier or lighter; that takes a rebuild.
//!
//! The other thing settled here is how an icon sits beside text. Lucide's
//! 24-unit box maps onto a whole em, so `size(16)` draws what a browser
//! would draw at `width="16"`, and the box straddles the middle of a capital
//! letter rather than resting on the baseline, so an icon next to a label
//! reads as level with it.

use kurbo::{
    Affine, BezPath, Cap, Circle, CubicBez, Ellipse, Join, Line, PathEl, Point,
    RoundedRect, Shape, Stroke, StrokeOpts,
};
use roxmltree::{Document, Node};
use std::error::Error;

/// Design units per em. A thousand is the usual choice for outlines drawn on
/// a round grid, and divides Lucide's 24 into an exact enough scale.
pub const UNITS_PER_EM: u16 = 1000;

/// Lucide draws every icon inside a 24x24 box, y running downwards.
const DRAWING_GRID: f64 = 24.0;

const DESIGN_UNITS_PER_GRID_UNIT: f64 = UNITS_PER_EM as f64 / DRAWING_GRID;

/// Pen width the icons are stroked at, in grid units. Lucide's own drawings
/// specify 2; JumpPad wants a heavier line. It is a proportion of the icon
/// like everything else in the glyph, so 3 here draws 2px at `size(16)`.
const STROKE_WIDTH: f64 = 3.0;

/// How much of an em a UI face typically puts between its baseline and the
/// top of its capitals. Only used to decide how far the icon box hangs below
/// the baseline, so a figure that suits most faces is what is wanted here
/// rather than a measurement of any one of them.
const CAP_HEIGHT: f64 = 0.7;

const BELOW_BASELINE: f64 = UNITS_PER_EM as f64 * (1.0 - CAP_HEIGHT) / 2.0;

/// How high the glyphs reach above the baseline, in design units.
pub const ASCENT: i16 = (UNITS_PER_EM as f64 - BELOW_BASELINE) as i16;

/// How far they hang below it. Negative, as font metrics have it.
pub const DESCENT: i16 = -(BELOW_BASELINE as i16);

/// How far a curve may stray from the shape it stands in for, in design
/// units. A twentieth of a percent of an em is finer than any rasterizer
/// will show.
const CURVE_TOLERANCE: f64 = 0.5;

/// Reads one Lucide drawing and returns the outline of its glyph, in design
/// units, made of the lines and quadratics a `glyf` entry is limited to.
pub fn outline(drawing: &str) -> Result<BezPath, Box<dyn Error>> {
    let centrelines = to_design_units(&stroke_centrelines(drawing)?);
    Ok(to_quadratics(&expand_stroke(&centrelines)))
}

/// The lines the drawing's pen travels along, still on Lucide's grid.
fn stroke_centrelines(drawing: &str) -> Result<Vec<BezPath>, Box<dyn Error>> {
    Document::parse(drawing)?
        .root_element()
        .children()
        .filter(Node::is_element)
        .map(shape_of)
        .collect()
}

fn shape_of(element: Node) -> Result<BezPath, Box<dyn Error>> {
    match element.tag_name().name() {
        "path" => Ok(BezPath::from_svg(text(&element, "d")?)?),
        "line" => Ok(Line::new(
            (number(&element, "x1")?, number(&element, "y1")?),
            (number(&element, "x2")?, number(&element, "y2")?),
        )
        .to_path(CURVE_TOLERANCE)),
        "circle" => Ok(Circle::new(
            (number(&element, "cx")?, number(&element, "cy")?),
            number(&element, "r")?,
        )
        .to_path(CURVE_TOLERANCE)),
        "ellipse" => Ok(Ellipse::new(
            (number(&element, "cx")?, number(&element, "cy")?),
            (number(&element, "rx")?, number(&element, "ry")?),
            0.0,
        )
        .to_path(CURVE_TOLERANCE)),
        "rect" => Ok(rectangle(&element)?.to_path(CURVE_TOLERANCE)),
        "polyline" => path_through(&corners(&element)?),
        "polygon" => path_through(&corners(&element)?).map(closed),
        other => Err(format!("unsupported SVG element <{other}>").into()),
    }
}

/// Lucide rounds a rectangle's corners with equal `rx` and `ry`, which is
/// the only kind of corner a [`RoundedRect`] has.
fn rectangle(element: &Node) -> Result<RoundedRect, Box<dyn Error>> {
    let left = number_or(element, "x", 0.0)?;
    let top = number_or(element, "y", 0.0)?;
    Ok(RoundedRect::new(
        left,
        top,
        left + number(element, "width")?,
        top + number(element, "height")?,
        number_or(element, "rx", 0.0)?,
    ))
}

fn corners(element: &Node) -> Result<Vec<Point>, Box<dyn Error>> {
    let numbers = text(element, "points")?
        .split(|character: char| character.is_whitespace() || character == ',')
        .filter(|piece| !piece.is_empty())
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()?;
    if numbers.len() % 2 != 0 {
        return Err("odd number of coordinates in points".into());
    }
    Ok(numbers
        .chunks_exact(2)
        .map(|pair| Point::new(pair[0], pair[1]))
        .collect())
}

fn path_through(corners: &[Point]) -> Result<BezPath, Box<dyn Error>> {
    let (&first, rest) = corners
        .split_first()
        .ok_or("points needs at least one coordinate pair")?;
    let mut path = BezPath::from_vec(vec![PathEl::MoveTo(first)]);
    for &corner in rest {
        path.line_to(corner);
    }
    Ok(path)
}

fn closed(mut path: BezPath) -> BezPath {
    path.close_path();
    path
}

fn text<'a>(
    element: &'a Node,
    attribute: &str,
) -> Result<&'a str, Box<dyn Error>> {
    element.attribute(attribute).ok_or_else(|| {
        format!("<{}> is missing its {attribute}", element.tag_name().name())
            .into()
    })
}

fn number(element: &Node, attribute: &str) -> Result<f64, Box<dyn Error>> {
    Ok(text(element, attribute)?.trim().parse()?)
}

fn number_or(
    element: &Node,
    attribute: &str,
    absent: f64,
) -> Result<f64, Box<dyn Error>> {
    match element.attribute(attribute) {
        Some(value) => Ok(value.trim().parse()?),
        None => Ok(absent),
    }
}

/// Scales the drawing up to the em, turns it the right way up, and drops it
/// so the box straddles the middle of a capital letter.
fn to_design_units(centrelines: &[BezPath]) -> BezPath {
    let mut placed: BezPath = centrelines
        .iter()
        .flat_map(|centreline| centreline.elements().iter().copied())
        .collect();
    placed.apply_affine(Affine::new([
        DESIGN_UNITS_PER_GRID_UNIT,
        0.0,
        0.0,
        -DESIGN_UNITS_PER_GRID_UNIT,
        0.0,
        f64::from(ASCENT),
    ]));
    placed
}

/// The outline a round pen of [`STROKE_WIDTH`] leaves travelling the
/// centrelines - a filled shape, which is all a glyph can be.
fn expand_stroke(centrelines: &BezPath) -> BezPath {
    let pen = Stroke::new(STROKE_WIDTH * DESIGN_UNITS_PER_GRID_UNIT)
        .with_caps(Cap::Round)
        .with_join(Join::Round);
    kurbo::stroke(centrelines, &pen, &StrokeOpts::default(), CURVE_TOLERANCE)
}

/// TrueType outlines hold lines and quadratics only, so the cubics that
/// stroking leaves behind for the round caps and joins are approximated.
fn to_quadratics(outline: &BezPath) -> BezPath {
    let mut quadratics = BezPath::new();
    let mut contour_start = Point::ZERO;
    let mut pen = Point::ZERO;
    for element in outline.elements() {
        match *element {
            PathEl::MoveTo(end) => {
                contour_start = end;
                pen = end;
                quadratics.move_to(end);
            }
            PathEl::LineTo(end) => {
                pen = end;
                quadratics.line_to(end);
            }
            PathEl::QuadTo(control, end) => {
                pen = end;
                quadratics.quad_to(control, end);
            }
            PathEl::CurveTo(first, second, end) => {
                let cubic = CubicBez::new(pen, first, second, end);
                for (_, _, quadratic) in cubic.to_quads(CURVE_TOLERANCE) {
                    quadratics.quad_to(quadratic.p1, quadratic.p2);
                }
                pen = end;
            }
            PathEl::ClosePath => {
                pen = contour_start;
                quadratics.close_path();
            }
        }
    }
    quadratics
}
