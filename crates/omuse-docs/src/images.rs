//! The workspace picture a document's `![alt](src)` / `<img src>` means, found in code. A model writing a report names
//! the chart it made a moment ago by hand and slips: `charts/monthly_sales_2026.png` for `charts/monthly-sales-2026.png`,
//! or a path relative to the workspace from a document in `reports/` (chart_real 2026-10-31: the PDF came out with no
//! pictures). Every renderer resolves pictures here, so none of them asks the model to fix a path and render again.

use std::path::{Component, Path, PathBuf};

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// `p` with `.` and `..` resolved, without touching the disk.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn inside(workspace: &Path, p: &Path) -> bool {
    p.starts_with(workspace) && p.is_file()
}

fn is_image(p: &Path) -> bool {
    p.extension()
        .map(|e| IMAGE_EXT.contains(&e.to_string_lossy().to_lowercase().as_str()))
        .unwrap_or(false)
}

/// A file name as compared: lower case, without `-`, `_`, spaces and dots before the extension.
fn key(name: &str) -> String {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let stem: String = stem
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' ' | '.'))
        .collect();
    format!("{stem}.{}", ext.to_lowercase().replace("jpeg", "jpg"))
}

/// The workspace image `src` refers to from a document in `base_dir`: the path as written (from the document's
/// folder, then from the workspace root), else the one image in the workspace whose file name is the same once case,
/// `-`, `_` and spaces are ignored, or a slip of it (`util::nearest`). `None` for a remote or unknown picture.
pub fn find_image(workspace: &Path, base_dir: &Path, src: &str) -> Option<PathBuf> {
    let src = src.trim();
    if src.is_empty() || src.contains("://") || src.starts_with("data:") {
        return None;
    }
    let rel = src.trim_start_matches('/');
    for p in [
        normalize(&base_dir.join(rel)),
        normalize(&workspace.join(rel)),
    ] {
        if inside(workspace, &p) && is_image(&p) {
            return Some(p);
        }
    }
    let want = key(Path::new(rel).file_name()?.to_string_lossy().as_ref());
    let images: Vec<PathBuf> = walkdir::WalkDir::new(workspace)
        .max_depth(5)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .filter(|p| p.is_file() && is_image(p))
        .take(5000)
        .collect();
    let keys: Vec<String> = images
        .iter()
        .map(|p| {
            key(&p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default())
        })
        .collect();
    let hit = omuse_common::util::nearest(&want, keys.iter().map(String::as_str))?;
    let mut at = keys
        .iter()
        .enumerate()
        .filter(|(_, k)| k.as_str() == hit)
        .map(|(i, _)| i);
    let first = at.next()?;
    // two pictures with that name in different folders: the one beside the path as written, else none
    match at.next() {
        None => Some(images[first].clone()),
        Some(_) => {
            let dir = normalize(&workspace.join(Path::new(rel).parent().unwrap_or(Path::new(""))));
            images
                .iter()
                .zip(&keys)
                .find(|(p, k)| k.as_str() == hit && p.parent() == Some(dir.as_path()))
                .map(|(p, _)| p.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_named_slightly_off_or_from_another_folder_is_found() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().canonicalize().unwrap();
        std::fs::create_dir_all(ws.join("charts")).unwrap();
        std::fs::create_dir_all(ws.join("reports")).unwrap();
        std::fs::write(ws.join("charts/monthly-sales-2026.png"), b"png").unwrap();
        std::fs::write(ws.join("charts/expense_approval_process.png"), b"png").unwrap();
        let reports = ws.join("reports");
        let chart = ws.join("charts/monthly-sales-2026.png");
        // chart_real 2026-10-31: underscores for hyphens, written from reports/ as if from the workspace root
        assert_eq!(
            find_image(&ws, &reports, "charts/monthly_sales_2026.png"),
            Some(chart.clone())
        );
        assert_eq!(
            find_image(&ws, &reports, "../charts/monthly-sales-2026.png"),
            Some(chart.clone())
        );
        assert_eq!(
            find_image(&ws, &reports, "Monthly Sales 2026.PNG"),
            Some(chart)
        );
        assert_eq!(
            find_image(&ws, &reports, "charts/expense-approval-proces.png"),
            Some(ws.join("charts/expense_approval_process.png"))
        );
        // nothing like it, outside the workspace, remote: none
        assert_eq!(find_image(&ws, &reports, "charts/revenue.png"), None);
        assert_eq!(find_image(&ws, &reports, "../../etc/passwd.png"), None);
        assert_eq!(find_image(&ws, &reports, "https://example.com/a.png"), None);
    }
}
