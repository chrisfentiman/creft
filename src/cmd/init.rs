use crate::error::CreftError;
use crate::model::AppContext;
use crate::store;

pub fn cmd_init(ctx: &AppContext) -> Result<(), CreftError> {
    let cwd = ctx.cwd.clone();

    if store::has_local_root(&cwd).is_some() {
        eprintln!("already initialized: {}", cwd.join(".creft").display());
        return Ok(());
    }

    if let Some(parent_root) = store::find_parent_local_root(&cwd) {
        eprintln!(
            "note: parent directory already has local skills at {}",
            parent_root.display()
        );
        eprintln!("creating nested .creft/ in current directory anyway");
    }

    let target = cwd.join(".creft").join("commands");
    std::fs::create_dir_all(&target).map_err(CreftError::Io)?;

    eprintln!("created: {}", target.display());
    Ok(())
}
