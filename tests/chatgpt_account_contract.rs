//! #327 static contract of the optional ChatGPT account: the repository files that the Android acceptance run cannot see.
//! Behavior is covered on the Android side (`AccountAcceptance`, `AccountTransportAcceptance`, `AccountUiAcceptance`).

use std::fs;
use std::path::PathBuf;

fn read(path: &str) -> String {
    let full = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

const ACCOUNT_SOURCES: [&str; 8] = [
    "AccountHttp.java",
    "AccountStore.java",
    "ChatGptAccount.java",
    "LoopbackListener.java",
    "Oidc.java",
    "PlanIndicatorView.java",
    "ReturnActivity.java",
    "SignInService.java",
];

// REQ-044.9
#[test]
fn req_044_9_project_goals_has_the_cloud_inference_subsection() {
    let goals = read("docs/project-goals.md");
    let section7 = goals
        .split("## 7. AI-assisted diagnostics")
        .nth(1)
        .and_then(|rest| rest.split("\n## 8.").next())
        .expect("section 7");
    assert!(section7.contains("### Cloud inference and accounts"));
    assert!(section7.contains(
        "Cloud inference and account use are optional and require explicit user consent. Local diagnostics, recording and replay remain usable without an account or network. Before sending diagnostic facts, show what will leave the device and minimize it; never send raw recordings or vehicle and device identifiers by default. Network access or AI output cannot elevate vehicle capabilities."
    ));
}

// REQ-044.9
#[test]
fn req_044_9_android_doc_describes_the_account_and_no_longer_denies_internet() {
    let doc = read("docs/android.md");
    assert!(doc.contains("## ChatGPT account (REQ-042, REQ-043, REQ-044)"));
    for needle in [
        "`INTERNET`",
        "SignInService",
        "FOREGROUND_SERVICE_DATA_SYNC",
        "REQ-044.6",
        "debug-no-signin-service",
    ] {
        assert!(doc.contains(needle), "docs/android.md lacks {needle}");
    }
    let lower = doc.to_lowercase();
    assert!(
        !lower.contains("no internet permission") && !lower.contains("no `internet`"),
        "docs/android.md still says the app has no internet permission"
    );
}

// REQ-044.3
#[test]
fn req_044_3_manifest_declares_internet_and_the_service_permission_on_single_lines() {
    let manifest = read("android/app/src/main/AndroidManifest.xml");
    assert!(
        manifest.contains("    <uses-permission android:name=\"android.permission.INTERNET\" />\n")
    );
    assert!(manifest.contains("    <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE_DATA_SYNC\" />\n"));
    for line in manifest.lines().filter(|l| l.contains("<uses-permission")) {
        assert!(
            line.contains("/>"),
            "uses-permission tag not on a single line: {line}"
        );
    }
    let permissions: Vec<&str> = manifest
        .lines()
        .filter(|l| l.contains("<uses-permission"))
        .collect();
    assert_eq!(permissions.len(), 11, "only INTERNET and the dataSync service permission were added to the nine existing permissions");
    assert!(manifest.contains("android:usesCleartextTraffic=\"false\""));
    assert!(manifest.contains("android:foregroundServiceType=\"dataSync\""));
    assert!(!manifest.contains("EXTERNAL_STORAGE"));
}

// REQ-044.3
#[test]
fn req_044_3_no_gradle_dependency_is_added() {
    for file in ["android/app/build.gradle", "android/build.gradle"] {
        let gradle = read(file);
        assert!(
            !gradle.contains("dependencies"),
            "{file} declares a dependency"
        );
        assert!(
            !gradle.contains("implementation") && !gradle.contains("compileOnly"),
            "{file} declares a library"
        );
    }
}

// REQ-044.3
#[test]
fn req_044_3_release_check_refuses_a_split_permission_tag() {
    let script = read("tools/verify-release-apk.sh");
    assert!(script.contains("uses-permission tag that is not on a single line"));
}

// REQ-044.4
#[test]
fn req_044_4_account_code_has_no_path_to_the_vehicle_session() {
    let forbidden = [
        "ObdService",
        "BleTransport",
        "NativeCore",
        "VehicleStore",
        "RecordingExporter",
        "PollSchedule",
        "ReconnectPolicy",
        "PendingActions",
    ];
    for file in ACCOUNT_SOURCES {
        let source = read(&format!("android/app/src/main/java/de/obdentic/app/{file}"));
        for name in forbidden {
            assert!(!source.contains(name), "{file} references {name}");
        }
    }
}

// REQ-044.2
#[test]
fn req_044_2_account_code_names_only_the_account_host() {
    let mut hosts = Vec::new();
    for file in ACCOUNT_SOURCES {
        let source = read(&format!("android/app/src/main/java/de/obdentic/app/{file}"));
        for token in source.split('"') {
            if let Some(rest) = token.strip_prefix("https://").filter(|r| !r.is_empty()) {
                hosts.push(rest.split('/').next().unwrap_or("").to_string());
            }
            assert!(
                !token.starts_with("http://") || token.starts_with("http://127.0.0.1"),
                "{file}: plain http URL {token}"
            );
        }
    }
    hosts.sort();
    hosts.dedup();
    assert_eq!(hosts, ["api.openai.com", "auth.openai.com", "chatgpt.com"]);
}
