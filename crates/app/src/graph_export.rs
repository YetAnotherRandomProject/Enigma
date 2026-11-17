//! Export du graphe en image :
//! - **SVG** : dessin vectoriel de tout le graphe (vue actuelle, même
//!   orientation en 3D), étiquettes comprises, net à toutes les tailles ;
//! - **PNG** : capture de la zone du graphe telle qu'affichée.

use std::fmt::Write as _;

use eframe::egui::{Color32, ColorImage, Rect};

/// Couleur SVG (`#rrggbb`) et opacité.
pub fn svg_color(c: Color32) -> (String, f32) {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    (format!("#{r:02x}{g:02x}{b:02x}"), f32::from(a) / 255.0)
}

/// Échappe un texte pour le SVG.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Petit constructeur de document SVG.
pub struct Svg {
    body: String,
}

impl Svg {
    pub fn new() -> Svg {
        Svg {
            body: String::new(),
        }
    }

    fn paint(attr: &str, c: Color32) -> String {
        let (hex, a) = svg_color(c);
        if a < 1.0 {
            format!(r#"{attr}="{hex}" {attr}-opacity="{a:.3}""#)
        } else {
            format!(r#"{attr}="{hex}""#)
        }
    }

    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, c: Color32) {
        let _ = writeln!(
            self.body,
            r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke-width="{width:.1}" {}/>"#,
            a.0,
            a.1,
            b.0,
            b.1,
            Self::paint("stroke", c)
        );
    }

    pub fn polygon(&mut self, pts: &[(f32, f32)], c: Color32) {
        let pts: Vec<String> = pts.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect();
        let _ = writeln!(
            self.body,
            r#"<polygon points="{}" {}/>"#,
            pts.join(" "),
            Self::paint("fill", c)
        );
    }

    pub fn circle(
        &mut self,
        c: (f32, f32),
        r: f32,
        fill: Option<Color32>,
        stroke: Option<(f32, Color32)>,
    ) {
        let fill = fill.map_or_else(|| r#"fill="none""#.to_owned(), |f| Self::paint("fill", f));
        let stroke = stroke.map_or(String::new(), |(w, s)| {
            format!(r#" stroke-width="{w:.1}" {}"#, Self::paint("stroke", s))
        });
        let _ = writeln!(
            self.body,
            r#"<circle cx="{:.1}" cy="{:.1}" r="{r:.1}" {fill}{stroke}/>"#,
            c.0, c.1
        );
    }

    /// Disque découpé en parts égales (une par couleur, à partir du haut).
    pub fn pie(&mut self, c: (f32, f32), r: f32, colors: &[Color32]) {
        if colors.len() <= 1 {
            self.circle(c, r, colors.first().copied(), None);
            return;
        }
        let n = colors.len() as f32;
        let per = std::f32::consts::TAU / n;
        for (k, color) in colors.iter().enumerate() {
            let a0 = -std::f32::consts::FRAC_PI_2 + per * k as f32;
            let a1 = a0 + per;
            let (x0, y0) = (c.0 + r * a0.cos(), c.1 + r * a0.sin());
            let (x1, y1) = (c.0 + r * a1.cos(), c.1 + r * a1.sin());
            let large = u8::from(per > std::f32::consts::PI);
            let _ = writeln!(
                self.body,
                r#"<path d="M{:.1},{:.1} L{x0:.1},{y0:.1} A{r:.1},{r:.1} 0 {large} 1 {x1:.1},{y1:.1} Z" {}/>"#,
                c.0,
                c.1,
                Self::paint("fill", *color)
            );
        }
    }

    pub fn text(&mut self, at: (f32, f32), size: f32, c: Color32, text: &str) {
        let _ = writeln!(
            self.body,
            r#"<text x="{:.1}" y="{:.1}" font-size="{size:.0}" text-anchor="middle" dominant-baseline="hanging" {}>{}</text>"#,
            at.0,
            at.1,
            Self::paint("fill", c),
            escape(text)
        );
    }

    /// Document complet ; `view` : zone (en coordonnées des éléments) à
    /// montrer, `bg` : couleur de fond.
    pub fn finish(self, view: Rect, bg: Color32) -> String {
        let (w, h) = (view.width().ceil(), view.height().ceil());
        let (bg_hex, _) = svg_color(bg);
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
             viewBox=\"{:.1} {:.1} {w} {h}\" font-family=\"Segoe UI, Ubuntu, Helvetica, Arial, sans-serif\">\n\
             <rect x=\"{:.1}\" y=\"{:.1}\" width=\"{w}\" height=\"{h}\" fill=\"{bg_hex}\"/>\n{}</svg>\n",
            view.left(),
            view.top(),
            view.left(),
            view.top(),
            self.body
        )
    }
}

/// Découpe `region` (en pixels de l'image) d'une capture d'écran et
/// l'encode en PNG.
pub fn encode_png_region(image: &ColorImage, region: Rect) -> Result<Vec<u8>, String> {
    let [iw, ih] = image.size;
    let x0 = (region.left().max(0.0) as usize).min(iw);
    let y0 = (region.top().max(0.0) as usize).min(ih);
    let x1 = (region.right().max(0.0) as usize).min(iw);
    let y1 = (region.bottom().max(0.0) as usize).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return Err("zone vide".to_owned());
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut data = Vec::with_capacity(w * h * 4);
    for y in y0..y1 {
        for x in x0..x1 {
            data.extend_from_slice(&image.pixels[y * iw + x].to_srgba_unmultiplied());
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&data).map_err(|e| e.to_string())?;
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, vec2};

    #[test]
    fn escapes_and_colors() {
        assert_eq!(escape("A & <b> \"c\""), "A &amp; &lt;b&gt; &quot;c&quot;");
        assert_eq!(svg_color(Color32::from_rgb(255, 0, 16)).0, "#ff0010");
        let (_, a) = svg_color(Color32::from_rgba_unmultiplied(0, 0, 0, 128));
        assert!((a - 0.502).abs() < 0.01);
    }

    #[test]
    fn svg_document_is_well_formed() {
        let mut svg = Svg::new();
        svg.line((0.0, 0.0), (10.0, 5.0), 1.5, Color32::RED);
        svg.pie((5.0, 5.0), 4.0, &[Color32::RED, Color32::BLUE]);
        svg.circle((5.0, 5.0), 4.0, None, Some((1.0, Color32::GREEN)));
        svg.text((5.0, 12.0), 12.0, Color32::WHITE, "Salle & coffre");
        let doc = svg.finish(
            Rect::from_min_size(pos2(-10.0, -10.0), vec2(40.0, 30.0)),
            Color32::BLACK,
        );
        assert!(doc.starts_with("<?xml"));
        assert!(doc.contains(r#"viewBox="-10.0 -10.0 40 30""#));
        assert_eq!(doc.matches("<path").count(), 2);
        assert!(doc.contains("Salle &amp; coffre"));
        assert!(doc.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn png_crop() {
        let mut img = ColorImage::filled([4, 3], Color32::BLACK);
        img.pixels[4 + 1] = Color32::RED; // (1, 1)
        let bytes =
            encode_png_region(&img, Rect::from_min_max(pos2(1.0, 1.0), pos2(3.0, 3.0))).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&buf[..4], &[255, 0, 0, 255]);
        assert!(
            encode_png_region(&img, Rect::from_min_max(pos2(5.0, 5.0), pos2(9.0, 9.0))).is_err()
        );
    }
}
