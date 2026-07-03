//! CRUD operations on workspace entities. All operations write straight to
//! disk; callers reload (or patch) the in-memory tree afterwards.

use std::path::{Path, PathBuf};

use crate::model::*;

use super::{
    io_err, load_json, save_json, slugify, validate_name, StoreError, StoreResult,
    COLLECTIONS_DIR, COLLECTION_FILE, ENVIRONMENTS_DIR, ENV_SUFFIX, FOLDER_FILE, REQUEST_SUFFIX,
};

pub fn save_request(file: &Path, def: &RequestDef) -> StoreResult<()> {
    save_json(file, def)
}

pub fn save_collection_meta(dir: &Path, meta: &CollectionMeta) -> StoreResult<()> {
    save_json(&dir.join(COLLECTION_FILE), meta)
}

pub fn save_folder_meta(dir: &Path, meta: &FolderMeta) -> StoreResult<()> {
    save_json(&dir.join(FOLDER_FILE), meta)
}

pub fn save_environment(file: &Path, env: &Environment) -> StoreResult<()> {
    save_json(file, env)
}

/// Create a new collection directory under `<root>/collections`.
pub fn create_collection(root: &Path, name: &str) -> StoreResult<PathBuf> {
    let slug = slugify(name);
    validate_name(&slug)?;
    let dir = root.join(COLLECTIONS_DIR).join(&slug);
    if dir.exists() {
        return Err(StoreError::AlreadyExists(dir));
    }
    std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    save_collection_meta(&dir, &CollectionMeta::new(name))?;
    Ok(dir)
}

/// Create a sub-folder inside a collection or folder directory.
pub fn create_folder(parent_dir: &Path, name: &str) -> StoreResult<PathBuf> {
    let slug = slugify(name);
    validate_name(&slug)?;
    let dir = parent_dir.join(&slug);
    if dir.exists() {
        return Err(StoreError::AlreadyExists(dir));
    }
    std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let meta = FolderMeta { name: name.to_string(), ..FolderMeta::default() };
    save_folder_meta(&dir, &meta)?;
    Ok(dir)
}

/// Create a new request file inside `parent_dir`, avoiding name collisions.
pub fn create_request(parent_dir: &Path, def: &RequestDef) -> StoreResult<PathBuf> {
    let slug = slugify(&def.name);
    validate_name(&slug)?;
    let mut file = parent_dir.join(format!("{slug}{REQUEST_SUFFIX}"));
    let mut counter = 2;
    while file.exists() {
        file = parent_dir.join(format!("{slug}-{counter}{REQUEST_SUFFIX}"));
        counter += 1;
    }
    save_request(&file, def)?;
    Ok(file)
}

/// Create a new environment (`<slug>.env.json`) under `<root>/environments`.
pub fn create_environment(root: &Path, name: &str) -> StoreResult<PathBuf> {
    let slug = slugify(name);
    validate_name(&slug)?;
    let file = root.join(ENVIRONMENTS_DIR).join(format!("{slug}{ENV_SUFFIX}"));
    if file.exists() {
        return Err(StoreError::AlreadyExists(file));
    }
    save_environment(&file, &Environment::new(name))?;
    Ok(file)
}

/// Rename a request file (updates the display name too) and patch the
/// parent's order array.
pub fn rename_request(file: &Path, new_name: &str) -> StoreResult<PathBuf> {
    let slug = slugify(new_name);
    validate_name(&slug)?;
    let mut def: RequestDef = load_json(file)?;
    def.name = new_name.to_string();
    let new_file = file.with_file_name(format!("{slug}{REQUEST_SUFFIX}"));
    if new_file != file && new_file.exists() {
        return Err(StoreError::AlreadyExists(new_file));
    }
    save_json(file, &def)?;
    if new_file != file {
        std::fs::rename(file, &new_file).map_err(io_err(file))?;
        rename_in_parent_order(file, &new_file)?;
    }
    Ok(new_file)
}

/// Rename a folder directory (display name + directory slug).
pub fn rename_folder(dir: &Path, new_name: &str) -> StoreResult<PathBuf> {
    let slug = slugify(new_name);
    validate_name(&slug)?;
    let meta_path = dir.join(FOLDER_FILE);
    let mut meta: FolderMeta =
        if meta_path.is_file() { load_json(&meta_path)? } else { FolderMeta::default() };
    meta.name = new_name.to_string();
    save_json(&meta_path, &meta)?;
    let new_dir = dir.with_file_name(&slug);
    if new_dir != dir {
        if new_dir.exists() {
            return Err(StoreError::AlreadyExists(new_dir));
        }
        std::fs::rename(dir, &new_dir).map_err(io_err(dir))?;
        rename_in_parent_order(dir, &new_dir)?;
    }
    Ok(new_dir)
}

/// Delete a request file and remove it from the parent's order array.
pub fn delete_request(file: &Path) -> StoreResult<()> {
    std::fs::remove_file(file).map_err(io_err(file))?;
    remove_from_parent_order(file)
}

/// Delete a folder or collection directory recursively.
pub fn delete_dir(dir: &Path) -> StoreResult<()> {
    std::fs::remove_dir_all(dir).map_err(io_err(dir))?;
    remove_from_parent_order(dir)
}

/// Duplicate a request file next to itself (`name-copy.request.json`).
pub fn duplicate_request(file: &Path) -> StoreResult<PathBuf> {
    let mut def: RequestDef = load_json(file)?;
    def.name = format!("{} copy", def.name);
    let parent = file.parent().unwrap_or(Path::new("."));
    create_request(parent, &def)
}

/// Move a request file into another directory (drag & drop between folders).
pub fn move_request(file: &Path, target_dir: &Path) -> StoreResult<PathBuf> {
    let name = file.file_name().unwrap_or_default();
    let target = target_dir.join(name);
    if target.exists() {
        return Err(StoreError::AlreadyExists(target));
    }
    std::fs::rename(file, &target).map_err(io_err(file))?;
    remove_from_parent_order(file)?;
    Ok(target)
}

/// Persist an explicit child ordering on a collection or folder directory.
pub fn set_order(dir: &Path, order: Vec<String>) -> StoreResult<()> {
    let col_meta = dir.join(COLLECTION_FILE);
    if col_meta.is_file() {
        let mut meta: CollectionMeta = load_json(&col_meta)?;
        meta.order = order;
        return save_json(&col_meta, &meta);
    }
    let folder_meta = dir.join(FOLDER_FILE);
    let mut meta: FolderMeta =
        if folder_meta.is_file() { load_json(&folder_meta)? } else { FolderMeta::default() };
    meta.order = order;
    save_json(&folder_meta, &meta)
}

fn parent_order_file(child: &Path) -> Option<(PathBuf, bool)> {
    let parent = child.parent()?;
    let col = parent.join(COLLECTION_FILE);
    if col.is_file() {
        return Some((col, true));
    }
    let folder = parent.join(FOLDER_FILE);
    if folder.is_file() {
        return Some((folder, false));
    }
    None
}

fn rename_in_parent_order(old: &Path, new: &Path) -> StoreResult<()> {
    patch_parent_order(old, |order| {
        let old_name = old.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let new_name = new.file_name().unwrap_or_default().to_string_lossy().into_owned();
        for entry in order.iter_mut() {
            if *entry == old_name {
                *entry = new_name.clone();
            }
        }
    })
}

fn remove_from_parent_order(child: &Path) -> StoreResult<()> {
    patch_parent_order(child, |order| {
        let name = child.file_name().unwrap_or_default().to_string_lossy().into_owned();
        order.retain(|e| *e != name);
    })
}

fn patch_parent_order(child: &Path, patch: impl FnOnce(&mut Vec<String>)) -> StoreResult<()> {
    let Some((meta_file, is_collection)) = parent_order_file(child) else {
        return Ok(());
    };
    if is_collection {
        let mut meta: CollectionMeta = load_json(&meta_file)?;
        patch(&mut meta.order);
        save_json(&meta_file, &meta)
    } else {
        let mut meta: FolderMeta = load_json(&meta_file)?;
        patch(&mut meta.order);
        save_json(&meta_file, &meta)
    }
}
