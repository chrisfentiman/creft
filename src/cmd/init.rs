use crate::error::CreftError;
use crate::model::AppContext;
use crate::store;

pub fn cmd_init(ctx: &AppContext) -> Result<(), CreftError> {
    let cwd = ctx.cwd.clone();

    if store::has_local_root(&cwd).is_some() {
        eprintln!("already initialized: {}", cwd.join(".creft").display());
        return Ok(());
    }

    // Explain how the new root interacts with any ancestor roots in the chain.
    let ancestor_roots = store::walk_parent_local_roots(&cwd);
    if !ancestor_roots.is_empty() {
        if ancestor_roots.len() == 1 {
            eprintln!(
                "note: ancestor .creft/ exists at {}",
                ancestor_roots[0].display()
            );
            eprintln!(
                "this .creft/ will overlay the existing root; closer scopes win on conflicts"
            );
        } else {
            eprintln!("note: ancestor .creft/ directories exist at:");
            for root in &ancestor_roots {
                eprintln!("  {}", root.display());
            }
            eprintln!("this .creft/ will overlay them; closer scopes win on conflicts");
        }
    }

    let target = cwd.join(".creft").join("commands");
    std::fs::create_dir_all(&target).map_err(CreftError::Io)?;

    eprintln!("created: {}", target.display());
    Ok(())
}
