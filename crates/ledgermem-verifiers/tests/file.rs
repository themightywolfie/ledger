//! The file verifier against a temporary directory.

mod common;

use common::{assert_observed, claim, verify};
use ledgermem_core::Outcome::{Fail, Inconclusive, Pass};
use ledgermem_verifiers::FileVerifier;
use serde_json::json;

const CONFIG: &str = "timeout: 30\nretries: 3\n";
/// The SHA-256 of [`CONFIG`].
const CONFIG_SHA256: &str = "76e0a46cff03cdadebf4ef9bc99f861f9326775ecfb35a27950ed7c2a897bbae";

/// A root holding `config/app.yaml`, and a verifier that reads under it.
fn root() -> (tempfile::TempDir, FileVerifier) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config").join("app.yaml"), CONFIG).unwrap();
    let verifier = FileVerifier::new(dir.path());
    (dir, verifier)
}

#[tokio::test]
async fn exists_passes_for_the_path_a_claim_names_and_fails_for_one_that_is_missing() {
    let (_dir, verifier) = root();
    let exists = json!({ "check": "exists" });

    let there = claim(
        "repo:acme/billing-api",
        "has_file",
        json!("config/app.yaml"),
    );
    let observed = verify(&verifier, &there, exists.clone()).await;
    assert_observed(&observed, Pass, "config/app.yaml exists");
    assert_eq!(observed.observed, json!(true));

    let missing = claim("repo:acme/billing-api", "has_file", json!("config/db.yaml"));
    let observed = verify(&verifier, &missing, exists).await;
    assert_observed(&observed, Fail, "config/db.yaml doesn't exist");
    assert_eq!(observed.observed, json!(false));
}

#[tokio::test]
async fn the_path_is_a_template_the_claim_fills_in() {
    let (_dir, verifier) = root();
    let file = claim("file:config/app.yaml", "file_present", json!(true));

    let observed = verify(
        &verifier,
        &file,
        json!({ "check": "exists", "path": "{subject_name}" }),
    )
    .await;

    assert_observed(&observed, Pass, "config/app.yaml exists");
}

#[tokio::test]
async fn contains_looks_for_the_claims_value_in_the_file() {
    let (_dir, verifier) = root();
    let contains = json!({ "check": "contains", "path": "config/app.yaml" });

    let set = claim("service:billing-api", "config_line", json!("timeout: 30"));
    assert_observed(
        &verify(&verifier, &set, contains.clone()).await,
        Pass,
        "config/app.yaml contains \"timeout: 30\"",
    );

    let unset = claim("service:billing-api", "config_line", json!("timeout: 60"));
    assert_observed(
        &verify(&verifier, &unset, contains).await,
        Fail,
        "config/app.yaml doesn't contain \"timeout: 60\"",
    );

    let missing = json!({ "check": "contains", "path": "config/db.yaml" });
    assert_observed(
        &verify(&verifier, &set, missing).await,
        Fail,
        "config/db.yaml doesn't exist",
    );
}

#[tokio::test]
async fn contains_looks_for_a_pattern_the_policy_sets_in_place_of_the_value() {
    let (_dir, verifier) = root();
    let retries = claim("service:billing-api", "retries", json!(3));

    let observed = verify(
        &verifier,
        &retries,
        json!({ "check": "contains", "path": "config/app.yaml", "pattern": "retries: {value}" }),
    )
    .await;

    assert_observed(&observed, Pass, "contains \"retries: 3\"");
}

#[tokio::test]
async fn sha256_compares_the_files_hash_with_the_claims_value() {
    let (_dir, verifier) = root();
    let sha256 = json!({ "check": "sha256", "path": "config/app.yaml" });

    for written in [
        CONFIG_SHA256.to_owned(),
        format!("SHA256:{}", CONFIG_SHA256.to_uppercase()),
    ] {
        let matching = claim("file:config/app.yaml", "sha256", json!(written));
        let observed = verify(&verifier, &matching, sha256.clone()).await;
        assert_observed(&observed, Pass, "config/app.yaml hashes to");
        assert_eq!(observed.observed, json!(format!("sha256:{CONFIG_SHA256}")));
    }

    let stale = claim("file:config/app.yaml", "sha256", json!("sha256:0000"));
    let observed = verify(&verifier, &stale, sha256).await;
    assert_observed(
        &observed,
        Fail,
        &format!("hashes to sha256:{CONFIG_SHA256}"),
    );

    let directory = json!({ "check": "sha256", "path": "config" });
    assert_observed(
        &verify(&verifier, &stale, directory).await,
        Fail,
        "config is a directory, not a file",
    );
}

#[tokio::test]
async fn a_path_that_would_leave_the_root_is_never_read() {
    let (dir, verifier) = root();
    let outside = dir.path().parent().unwrap().join("outside.txt");
    let escapes = [
        "../outside.txt".to_owned(),
        "config/../../outside.txt".to_owned(),
        outside.display().to_string(),
    ];

    for path in escapes {
        let observed = verify(
            &verifier,
            &claim("file:x", "file_present", json!(true)),
            json!({ "check": "exists", "path": path }),
        )
        .await;
        assert_observed(
            &observed,
            Inconclusive,
            "isn't a path under the file verifier's root",
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_that_leads_out_of_the_root_is_never_read() {
    let (dir, verifier) = root();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "hunter2").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("linked")).unwrap();

    let observed = verify(
        &verifier,
        &claim("file:linked/secret.txt", "secret", json!("hunter2")),
        json!({ "check": "contains", "path": "{subject_name}" }),
    )
    .await;

    assert_observed(
        &observed,
        Inconclusive,
        "leads out of the file verifier's root",
    );

    // Nor can a link to nothing tell whether a file outside the root exists.
    std::os::unix::fs::symlink(
        outside.path().join("missing.txt"),
        dir.path().join("dangling"),
    )
    .unwrap();
    let observed = verify(
        &verifier,
        &claim("file:dangling", "file_present", json!(true)),
        json!({ "check": "exists", "path": "{subject_name}" }),
    )
    .await;
    assert_observed(
        &observed,
        Inconclusive,
        "dangling leads through a link to nothing",
    );
}

#[tokio::test]
async fn a_path_through_a_file_does_not_exist() {
    let (_dir, verifier) = root();
    let observed = verify(
        &verifier,
        &claim(
            "repo:acme/billing-api",
            "has_file",
            json!("config/app.yaml/extra"),
        ),
        json!({ "check": "exists" }),
    )
    .await;

    assert_observed(&observed, Fail, "config/app.yaml/extra doesn't exist");
}

#[tokio::test]
async fn a_check_the_file_verifier_does_not_have_is_inconclusive() {
    let (_dir, verifier) = root();
    let any = claim("file:config/app.yaml", "file_present", json!(true));

    for params in [
        json!({ "check": "mtime", "path": "config/app.yaml" }),
        json!({ "path": "config/app.yaml" }),
        json!({ "check": "contains" }),
    ] {
        let observed = verify(&verifier, &any, params).await;
        assert_eq!(observed.outcome, Inconclusive, "{observed:?}");
    }
}
