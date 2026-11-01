//! Local-only font acceptance: paths are supplied by the operator, never embedded.
use cadcraft_fonts::{TextFont, shx};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [regular, big, height, width, rest @ ..] = args.as_slice() else {
        return Err("usage: shx_measure REGULAR BIG HEIGHT WIDTH TEXT ...".into());
    };
    let regular = shx::find(regular).ok_or("regular SHX unavailable")?;
    let big = shx::find(big).ok_or("big SHX unavailable")?;
    let font = TextFont::Shx(shx::Pair { regular, big: Some(big), encoding: shx::Encoding::Big5 });
    for s in rest {
        println!("{:.10}", cadcraft_fonts::text_width(&font, s, height.parse()?, width.parse()?));
    }
    Ok(())
}
