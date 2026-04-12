//! Tests for `creft rm`.

mod helpers;

use helpers::{creft_env, creft_with};
use predicates::prelude::*;

/// `creft rm <pkg> <skill>` against a packages/ skill is rejected with an
/// actionable error directing the user to uninstall the whole package instead.
#[test]
fn rm_package_skill_is_rejected() {
    let creft_home = creft_env();
    let pkg_dir = creft_home.path().join("packages").join("rm-guard-pkg");
    std::fs::create_dir_all(pkg_dir.join(".creft")).unwrap();

    std::fs::write(
        pkg_dir.join(".creft").join("catalog.json"),
        r#"{"name":"rm-guard-pkg","description":"rm guard test package","plugins":[{"name":"rm-guard-pkg","source":".","description":"rm guard test package","version":"1.0.0","tags":[]}]}"#,
    )
    .unwrap();
    std::fs::write(
        pkg_dir.join("build.md"),
        "---\nname: build\ndescription: builds the thing\n---\n\n```bash\necho building\n```\n",
    )
    .unwrap();

    creft_with(&creft_home)
        .args(["cmd", "rm", "rm-guard-pkg", "build"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("cannot remove"));
}
