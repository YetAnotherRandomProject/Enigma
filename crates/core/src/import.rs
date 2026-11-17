//! Import de fiches `.md` (fichiers isolés ou dossier entier) dans un
//! projet, et gestion des images du projet (`assets/`).
//!
//! Les images locales référencées par une fiche importée sont copiées dans
//! `assets/` du projet et leurs chemins réécrits ; les adresses web restent
//! telles quelles. Les fichiers d'origine ne sont jamais modifiés.

use std::path::{Path, PathBuf};

use crate::project::META_DIR;
use crate::render::images;
use crate::storage::{file_stem_for_title, unique_note_path, write_atomic};

/// Dossier des images d'un projet.
pub const ASSETS_DIR: &str = "assets";

/// Extensions d'images prises en charge.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("impossible de {action} {path} : {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("« {0} » n'est pas une image prise en charge (png, jpg, gif, webp, svg)")]
    NotAnImage(PathBuf),
}

fn io_err(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> ImportError {
    let path = path.to_owned();
    move |source| ImportError::Io {
        action,
        path,
        source,
    }
}

/// Bilan d'un import.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Fiches importées (chemins dans le projet).
    pub notes: Vec<PathBuf>,
    /// Images copiées.
    pub images: usize,
    /// Images référencées mais introuvables (laissées telles quelles).
    pub missing: Vec<String>,
}

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// Adresse web ou donnée intégrée (pas un fichier local) ?
pub fn is_remote(src: &str) -> bool {
    src.contains("://") || src.starts_with("data:")
}

/// Décodage minimal des `%XX` d'un chemin Markdown.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = bytes.get(i + 1..i + 3)
            && let Ok(hex) = std::str::from_utf8(h)
            && let Ok(v) = u8::from_str_radix(hex, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Copie une image dans `assets/` du projet sous un nom libre ; rend son
/// chemin relatif au projet (`assets/plan 2.png`).
pub fn add_asset(project_root: &Path, source: &Path) -> Result<String, ImportError> {
    if !is_image_path(source) {
        return Err(ImportError::NotAnImage(source.to_owned()));
    }
    let bytes = std::fs::read(source).map_err(io_err("lire", source))?;
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image.png".to_owned());
    add_asset_bytes(project_root, &name, &bytes)
}

/// Comme [`add_asset`], à partir d'octets (presse-papiers, glisser-déposer).
pub fn add_asset_bytes(
    project_root: &Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<String, ImportError> {
    let dir = project_root.join(ASSETS_DIR);
    std::fs::create_dir_all(&dir).map_err(io_err("créer", &dir))?;
    let path = Path::new(file_name);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "png".to_owned());
    let stem = file_stem_for_title(
        &path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    let mut name = format!("{stem}.{ext}");
    let mut k = 2;
    while dir.join(&name).exists() {
        name = format!("{stem} {k}.{ext}");
        k += 1;
    }
    let target = dir.join(&name);
    write_atomic(&target, bytes).map_err(io_err("écrire", &target))?;
    Ok(format!("{ASSETS_DIR}/{name}"))
}

/// Copie dans le projet les images locales d'un texte situées hors du
/// projet, et réécrit leurs chemins. `note_dir` : dossier d'origine du
/// texte (pour les chemins relatifs).
pub fn localize_images(
    text: &str,
    note_dir: &Path,
    project_root: &Path,
    report: &mut ImportReport,
) -> Result<String, ImportError> {
    let mut out = text.to_owned();
    // De la fin vers le début : les positions restent valables.
    for img in images(text).into_iter().rev() {
        if is_remote(&img.src) || img.src.is_empty() {
            continue;
        }
        let decoded = percent_decode(&img.src);
        let source = if Path::new(&decoded).is_absolute() {
            PathBuf::from(&decoded)
        } else {
            note_dir.join(&decoded)
        };
        if !source.is_file() {
            report.missing.push(img.src.clone());
            continue;
        }
        let rel = add_asset(project_root, &source)?;
        report.images += 1;
        let Some(written) = out.get(img.range.clone()) else {
            continue;
        };
        let Some(close) = written.find("](") else {
            continue;
        };
        let alt_part = written[..close + 2].to_owned();
        let new_src = if rel.contains([' ', '(', ')']) {
            format!("<{rel}>")
        } else {
            rel
        };
        out.replace_range(img.range.clone(), &format!("{alt_part}{new_src})"));
    }
    Ok(out)
}

/// Importe des fichiers `.md` à la racine du projet (nom libre choisi
/// automatiquement), avec leurs images.
pub fn import_files(project_root: &Path, files: &[PathBuf]) -> Result<ImportReport, ImportError> {
    let mut report = ImportReport::default();
    for file in files {
        let text = std::fs::read_to_string(file).map_err(io_err("lire", file))?;
        let note_dir = file.parent().unwrap_or(Path::new("."));
        let text = localize_images(&text, note_dir, project_root, &mut report)?;
        let stem = file_stem_for_title(
            &file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        let target = unique_note_path(project_root, &stem, Path::exists);
        write_atomic(&target, text.as_bytes()).map_err(io_err("écrire", &target))?;
        report.notes.push(target);
    }
    Ok(report)
}

/// Importe tout un dossier de fiches (ancien format) dans un projet vide :
/// fiches et sous-dossiers, images, réglages `.enigma/` (pack, tableau,
/// packs locaux). Les dossiers cachés autres que `.enigma` sont ignorés ;
/// les images référencées hors du dossier sont copiées dans `assets/`.
pub fn import_folder(source: &Path, project_root: &Path) -> Result<ImportReport, ImportError> {
    let mut report = ImportReport::default();
    let walker = walkdir::WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0 || {
                let name = e.file_name().to_string_lossy();
                !name.starts_with('.') || (e.depth() == 1 && name == META_DIR)
            }
        });
    for item in walker {
        let item = item.map_err(|e| ImportError::Io {
            action: "parcourir",
            path: source.to_owned(),
            source: e.into(),
        })?;
        if !item.file_type().is_file() {
            continue;
        }
        let Ok(rel) = item.path().strip_prefix(source) else {
            continue;
        };
        let target = project_root.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io_err("créer", parent))?;
        }
        let is_md = item
            .path()
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"));
        let in_meta = rel.starts_with(META_DIR);
        if is_md && !in_meta {
            let text = std::fs::read_to_string(item.path()).map_err(io_err("lire", item.path()))?;
            let note_dir = item.path().parent().unwrap_or(source);
            let fixed = localize_outside(&text, note_dir, source, project_root, &mut report)?;
            write_atomic(&target, fixed.as_bytes()).map_err(io_err("écrire", &target))?;
            report.notes.push(target);
        } else {
            std::fs::copy(item.path(), &target).map_err(io_err("copier", item.path()))?;
            if is_image_path(item.path()) {
                report.images += 1;
            }
        }
    }
    Ok(report)
}

/// Comme [`localize_images`], mais seulement pour les images situées hors
/// du dossier importé (celles du dossier sont copiées avec lui).
fn localize_outside(
    text: &str,
    note_dir: &Path,
    source_root: &Path,
    project_root: &Path,
    report: &mut ImportReport,
) -> Result<String, ImportError> {
    let root = source_root.canonicalize().ok();
    let outside = images(text).into_iter().any(|img| {
        if is_remote(&img.src) {
            return false;
        }
        let p = note_dir.join(percent_decode(&img.src));
        match (p.canonicalize(), &root) {
            (Ok(c), Some(r)) => !c.starts_with(r),
            _ => true,
        }
    });
    if !outside {
        // Aucune image hors du dossier, mais signaler les manquantes.
        for img in images(text) {
            if !is_remote(&img.src) && !note_dir.join(percent_decode(&img.src)).is_file() {
                report.missing.push(img.src);
            }
        }
        return Ok(text.to_owned());
    }
    localize_images(text, note_dir, project_root, report)
}

/// Chemin d'une image d'une fiche : relatif au dossier de la fiche, sinon
/// à la racine du projet. `None` pour une adresse web ou un fichier absent.
pub fn resolve_image(project_root: &Path, note_path: &Path, src: &str) -> Option<PathBuf> {
    if is_remote(src) {
        return None;
    }
    let decoded = percent_decode(src);
    let rel = Path::new(&decoded);
    if rel.is_absolute() {
        return rel.is_file().then(|| rel.to_owned());
    }
    let near = note_path.parent().map(|d| d.join(rel));
    near.filter(|p| p.is_file())
        .or_else(|| Some(project_root.join(rel)).filter(|p| p.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, content: &[u8]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("mon%20plan.png"), "mon plan.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%C3%A9t%C3%A9"), "été");
    }

    #[test]
    fn assets_get_unique_names() {
        let p = tempfile::tempdir().unwrap();
        let a = add_asset_bytes(p.path(), "Plan.PNG", b"1").unwrap();
        let b = add_asset_bytes(p.path(), "Plan.png", b"2").unwrap();
        assert_eq!(a, "assets/Plan.png");
        assert_eq!(b, "assets/Plan 2.png");
        assert!(matches!(
            add_asset(p.path(), Path::new("notes.txt")),
            Err(ImportError::NotAnImage(_))
        ));
    }

    #[test]
    fn import_files_copies_images_and_rewrites() {
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("img/plan du manoir.svg"), b"<svg/>");
        write(
            &src.path().join("Salle.md"),
            "Voir ![Plan|300](img/plan%20du%20manoir.svg), ![web](https://x/y.png) et ![x](absente.png)."
                .as_bytes(),
        );
        let proj = tempfile::tempdir().unwrap();
        write(&proj.path().join("Salle.md"), b"existe deja");
        let report = import_files(proj.path(), &[src.path().join("Salle.md")]).unwrap();
        assert_eq!(report.notes, [proj.path().join("Salle 2.md")]);
        assert_eq!(report.images, 1);
        assert_eq!(report.missing, ["absente.png"]);
        let text = std::fs::read_to_string(proj.path().join("Salle 2.md")).unwrap();
        assert!(
            text.contains("![Plan|300](<assets/plan du manoir.svg>)"),
            "{text}"
        );
        assert!(text.contains("https://x/y.png"));
        assert_eq!(
            std::fs::read(proj.path().join("assets/plan du manoir.svg")).unwrap(),
            b"<svg/>"
        );
        let resolved = resolve_image(
            proj.path(),
            &proj.path().join("Salle 2.md"),
            "assets/plan du manoir.svg",
        );
        assert!(resolved.is_some());
    }

    #[test]
    fn import_folder_keeps_structure_and_settings() {
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("ext.png"), b"png");
        let src = tempfile::tempdir().unwrap();
        write(&src.path().join("A.md"), b"![a](images/a.png)");
        write(&src.path().join("images/a.png"), b"a");
        let ext = outside.path().join("ext.png");
        write(
            &src.path().join("sous/B.md"),
            format!("![e]({})", ext.display().to_string().replace('\\', "/")).as_bytes(),
        );
        write(&src.path().join(".enigma/project.json"), b"{}");
        write(&src.path().join(".git/HEAD"), b"x");
        let proj = tempfile::tempdir().unwrap();
        let report = import_folder(src.path(), proj.path()).unwrap();
        assert_eq!(report.notes.len(), 2);
        assert!(proj.path().join("images/a.png").is_file());
        assert!(proj.path().join(".enigma/project.json").is_file());
        assert!(!proj.path().join(".git").exists());
        let b = std::fs::read_to_string(proj.path().join("sous/B.md")).unwrap();
        assert_eq!(b, "![e](assets/ext.png)");
        assert_eq!(
            std::fs::read_to_string(proj.path().join("A.md")).unwrap(),
            "![a](images/a.png)",
            "images du dossier : chemins inchangés"
        );
    }
}
