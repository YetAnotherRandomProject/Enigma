//! `enigma-typeset` : composition avec Typst (formules mathématiques en
//! SVG, export d'une fiche en PDF) et diagrammes Mermaid en SVG. Aucune
//! dépendance à l'interface.
//!
//! - [`math_svg`] : formule LaTeX (`$…$`, `$$…$$`) → SVG (glyphes en
//!   tracés, aucune police nécessaire pour l'afficher).
//! - [`mermaid_svg`] : diagramme Mermaid → SVG, texte converti en tracés.
//! - [`note_pdf`] : fiche Markdown → PDF (titres, listes, tableaux,
//!   images, formules, diagrammes, encadrés).
//!
//! Les polices viennent de `typst-assets` (intégrées à l'exécutable).

mod latex;
mod pdf;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

pub use latex::latex_to_typst;
pub use pdf::{markdown_to_typst, note_pdf};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TypesetError {
    #[error("formule : {0}")]
    Math(String),
    #[error("diagramme : {0}")]
    Mermaid(String),
    #[error("PDF : {0}")]
    Pdf(String),
}

/// Polices et bibliothèque, chargées une fois.
struct Shared {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| {
        let fonts: Vec<Font> = typst_assets::fonts()
            .flat_map(|data| Font::iter(Bytes::new(data)))
            .collect();
        Shared {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(FontBook::from_fonts(&fonts)),
            fonts,
        }
    })
}

fn file_id(path: &str) -> Option<FileId> {
    let vpath = VirtualPath::new(path).ok()?;
    Some(FileId::new(RootedPath::new(VirtualRoot::Project, vpath)))
}

/// Monde Typst minimal : un fichier principal et des fichiers en mémoire
/// (images, diagrammes).
pub(crate) struct MemWorld {
    main: Source,
    files: HashMap<FileId, Bytes>,
}

impl MemWorld {
    pub(crate) fn new(text: String, files: Vec<(String, Vec<u8>)>) -> Option<MemWorld> {
        let main = Source::new(file_id("/main.typ")?, text);
        let files = files
            .into_iter()
            .filter_map(|(p, data)| Some((file_id(&p)?, Bytes::new(data))))
            .collect();
        Some(MemWorld { main, files })
    }

    pub(crate) fn compile(&self) -> Result<PagedDocument, String> {
        typst::compile::<PagedDocument>(self)
            .output
            .map_err(|errs| {
                errs.iter()
                    .map(|e| e.message.to_string())
                    .collect::<Vec<_>>()
                    .join(" ; ")
            })
    }
}

impl World for MemWorld {
    fn library(&self) -> &LazyHash<Library> {
        &shared().library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &shared().book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::NotSource)
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files
            .get(&id)
            .cloned()
            .ok_or_else(|| FileError::NotFound(id.vpath().get_without_slash().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        shared().fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<typst::foundations::Duration>) -> Option<Datetime> {
        None
    }
}

/// Formule LaTeX → SVG. `color` : `#rrggbb` ; `size` : taille du texte en
/// points ; `display` : formule centrée (`$$…$$`).
pub fn math_svg(
    latex: &str,
    display: bool,
    color: &str,
    size: f32,
) -> Result<String, TypesetError> {
    let math = latex_to_typst(latex);
    let gap = if display { " " } else { "" };
    let src = format!(
        "#set page(width: auto, height: auto, margin: 1pt, fill: none)\n\
         #set text(size: {size}pt, fill: rgb(\"{color}\"))\n\
         ${gap}{math}{gap}$"
    );
    let world = MemWorld::new(src, Vec::new()).ok_or(TypesetError::Math("chemin".into()))?;
    let doc = world.compile().map_err(TypesetError::Math)?;
    let page = doc
        .pages()
        .first()
        .ok_or_else(|| TypesetError::Math("aucune page".into()))?;
    Ok(typst_svg::svg(page, &typst_svg::SvgOptions::default()))
}

/// Diagramme Mermaid → SVG (texte converti en tracés).
pub fn mermaid_svg(src: &str, dark: bool) -> Result<String, TypesetError> {
    let opts = mermaid_rs_renderer::RenderOptions {
        theme: if dark {
            mermaid_rs_renderer::Theme::dark()
        } else {
            mermaid_rs_renderer::Theme::modern()
        },
        ..Default::default()
    };
    let svg = mermaid_rs_renderer::render_with_options(src, opts)
        .map_err(|e| TypesetError::Mermaid(e.to_string()))?;
    flatten_text(&svg).map_err(TypesetError::Mermaid)
}

/// Base de polices pour le texte des SVG : polices du système, et celles
/// de Typst en secours.
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        for data in typst_assets::fonts() {
            db.load_font_data(data.to_vec());
        }
        db.set_sans_serif_family("DejaVu Sans");
        Arc::new(db)
    })
    .clone()
}

/// Réécrit un SVG en convertissant le texte en tracés.
fn flatten_text(svg: &str) -> Result<String, String> {
    let opt = usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    Ok(tree.to_string(&usvg::WriteOptions::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_math_to_svg() {
        let svg = math_svg(r"\frac{a}{b} + \sqrt{x^2}", false, "#202020", 14.0).unwrap();
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(svg.contains("<path") || svg.contains("<use"));
        // Une formule fautive donne une erreur lisible, pas une panique.
        assert!(matches!(
            math_svg(r"\frac{a", true, "#000000", 12.0),
            Ok(_) | Err(TypesetError::Math(_))
        ));
    }

    #[test]
    fn renders_mermaid_to_svg() {
        let svg = mermaid_svg("flowchart LR\n  A[Clé] --> B[Coffre]", false).unwrap();
        assert!(
            svg.contains("<svg") && !svg.contains("<text"),
            "texte en tracés"
        );
        assert!(mermaid_svg("pas un diagramme ][", true).is_err());
    }
}
