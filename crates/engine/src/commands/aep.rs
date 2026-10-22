//! Import editable After Effects project data into the current EffectCraft project.

use serde_json::{Value, json};

use super::{CommandSpec, always, b_p, bad, str_p};
use crate::{EngineError, Result, Session, cmd};

fn import_document(s: &mut Session, path: &str, bytes: &[u8], dir: &str, allow_losses: bool) -> Result<Value> {
    let services = s.services.clone();
    let mut resolve_media = |raw: &str| {
        let path = std::path::Path::new(raw);
        if services.exists(raw) {
            Some(raw.to_string())
        } else if !path.is_absolute() {
            let relative = std::path::Path::new(dir).join(path).to_string_lossy().to_string();
            services.exists(&relative).then_some(relative)
        } else {
            None
        }
    };
    // The edit works on a copy of the project and drops it on an error: no second copy needed.
    let imported = s.edit("Import After Effects Project", None, |project, state| {
        let result = effectcraft_aep::import_ir_into(project, bytes, dir, &mut resolve_media).map_err(|e| EngineError::Other(e.to_string()))?;
        if !allow_losses && !result.losses.is_empty() {
            return Err(EngineError::Other(format!("AEP import has {} fidelity gaps; project left unchanged", result.losses.len())));
        }
        state.project_selection = result.comps.clone();
        Ok(result)
    })?;
    if let Some(comp) = imported.primary_comp.or_else(|| imported.comps.first().copied()) {
        s.open_comp(comp);
    }
    if !imported.losses.is_empty() {
        s.toast(format!("Imported {path} with {} fidelity gaps", imported.losses.len()));
        log::warn!("AEP import {path}: {} fidelity gaps", imported.losses.len());
        for loss in &imported.losses {
            log::warn!("  {loss}");
        }
    }
    Ok(json!({
        "comps": imported.comps.iter().map(|id| id.0).collect::<Vec<_>>(),
        "primaryComp": imported.primary_comp.map(|id| id.0),
        "items": imported.items.iter().map(|id| id.0).collect::<Vec<_>>(),
        "missingMedia": imported.missing_media,
        "losses": imported.losses,
        "warnings": imported.warnings,
    }))
}

fn import_ir(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.importAeIr", "missing `path`"))?.to_string();
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let dir =
        str_p(p, "sourceDir").map(str::to_string).or_else(|| std::path::Path::new(&path).parent().map(|p| p.to_string_lossy().to_string())).unwrap_or_default();
    import_document(s, &path, &bytes, &dir, b_p(p, "allowLosses").unwrap_or(false))
}

/// An `.aep` opened or imported from the app (Open, drops, Finder): read in the background and
/// imported even with fidelity gaps, which the toast counts and the log lists.
pub(crate) fn import_picked(s: &mut Session, path: &str) -> Result<Value> {
    import_aep(s, &json!({"path": path, "wait": false, "allowLosses": true}))
}

fn import_aep(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("file.importAep", "missing `path`"))?.to_string();
    if !path.to_ascii_lowercase().ends_with(".aep") {
        return Err(bad("file.importAep", "expected an After Effects .aep project"));
    }
    let dir = std::path::Path::new(&path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let allow_losses = b_p(p, "allowLosses").unwrap_or(false);
    // `wait: false`: the export (the After Effects reader, minutes for a big project) runs on a
    // background thread; the import applies on the session's thread when it is done.
    if b_p(p, "wait") == Some(false) {
        let services = s.services.clone();
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        return s.spawn_task("importAep", format!("Importing {name}"), false, move |ctl| {
            ctl.message("Opening the After Effects project");
            let progress = |done, total, message: &str| {
                ctl.message(message);
                ctl.progress(done, total)
            };
            let bytes = services.export_aep_ir(&path, &progress).map_err(|e| format!("cannot import {path}: {e}"))?;
            ctl.message("Building the project");
            let apply: crate::jobs::Apply = Box::new(move |s: &mut Session| import_document(s, &path, &bytes, &dir, allow_losses));
            Ok(apply)
        });
    }
    let bytes = s.services.export_aep_ir(&path, &|_, _, _| true).map_err(|e| EngineError::Other(format!("cannot import {path}: {e}")))?;
    import_document(s, &path, &bytes, &dir, allow_losses)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.importAep",
            "After Effects Project...",
            ["File", "Import"],
            None,
            "{path (.aep), allowLosses?: bool (inspect incomplete import), wait? (false: export in the background)}",
            always,
            import_aep
        ),
        cmd!(
            "file.importAeIr",
            "After Effects Project Data...",
            [],
            None,
            "{path (.json), sourceDir? (AEP project folder for relative media), allowLosses?: bool}",
            always,
            import_ir
        ),
    ]
}
