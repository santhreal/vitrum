use super::*;
use crate::testkit::info;
use vitrum_proto::ProjectId;

fn project(id: u64, root: &str) -> ProjectInfo {
    ProjectInfo {
        id: ProjectId(id),
        name: root.rsplit('/').next().unwrap_or(root).to_string(),
        root: root.to_string(),
    }
}

/// A directory reached through a symlink is the SAME project. Four
/// sessions started in one repo, one of them via a symlinked path, drew
/// four sidebar groups all called `vitrum` holding one session each; the
/// symlink is the case no amount of string comparison can fix.
#[test]
fn a_symlinked_root_keys_to_the_directory_it_points_at() {
    let base = std::env::temp_dir().join(format!("vitrum-pk-{}", std::process::id()));
    let real = base.join("real");
    let link = base.join("link");
    std::fs::create_dir_all(&real).expect("temp dir");
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real, &link).expect("symlink");

    let direct = project_key(real.to_str().unwrap());
    let through_link = project_key(link.to_str().unwrap());
    assert_eq!(
        direct, through_link,
        "a symlink to a directory must key as that directory"
    );
    assert!(
        direct.ends_with("real"),
        "the key must be the target, not the link: {direct}"
    );

    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir_all(&base);
}

/// A trailing separator is typing, not identity. `/src/vitrum/` and
/// `/src/vitrum` are one directory, and a client that appends one must not
/// mint a second project.
#[test]
fn a_trailing_separator_keys_the_same_directory() {
    let dir = std::env::temp_dir().join(format!("vitrum-sep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let plain = dir.to_str().unwrap().to_string();

    assert_eq!(project_key(&plain), project_key(&format!("{plain}/")));
    assert_eq!(project_key(&plain), project_key(&format!("{plain}///")));
    assert_eq!(project_key(&plain), project_key(&format!("  {plain}  ")));
    let _ = std::fs::remove_dir_all(&dir);

    // The same has to hold with no filesystem to ask, which is the branch
    // a cwd that has been deleted underneath a session lands in.
    let gone = "/vitrum-does-not-exist/deep";
    assert_eq!(
        project_key(gone),
        project_key("/vitrum-does-not-exist/deep/")
    );
    assert_eq!(
        project_key(gone),
        project_key(" /vitrum-does-not-exist/deep// ")
    );
    assert_eq!(
        project_key("/"),
        "/",
        "the root is a separator, not padding"
    );
}

/// Case is identity on Linux and is not on macOS or Windows, and the key
/// has to follow the platform rather than pick one and be wrong on two of
/// three. Locked for the nonexistent-path branch, which is the only branch
/// that decides case for itself: when the path exists the OS answers, and
/// on a case-insensitive volume it hands back one on-disk spelling for
/// both.
#[test]
fn case_folding_follows_the_platform() {
    let upper = project_key("/vitrum-nonexistent/Dev");
    let lower = project_key("/vitrum-nonexistent/dev");
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        assert_eq!(
            upper, lower,
            "a case-insensitive filesystem has one directory here"
        );
    } else {
        assert_ne!(
            upper, lower,
            "on Linux these are two directories and merging them hides a project"
        );
    }
}

/// THE DEFECT. Four daemon project ids for one root must fold to one
/// group, and every session must still find its way into it whichever id
/// it was created under.
#[test]
fn one_directory_is_one_project_however_many_ids_the_daemon_minted() {
    let projects = vec![
        project(11, "/src/vitrum"),
        project(22, "/src/vitrum/"),
        project(33, "/src/vitrum"),
        project(44, "/src/other"),
    ];
    let folded = coalesce_projects(&projects);

    assert_eq!(folded.groups().len(), 2, "one group per directory");
    assert_eq!(folded.groups()[0].key, project_key("/src/vitrum"));
    assert_eq!(
        folded.groups()[0].lead.id,
        ProjectId(11),
        "the header keeps the first record, so a second client cannot rename it"
    );
    assert_eq!(
        folded.groups()[0].id,
        ProjectId(fnv1a(project_key("/src/vitrum").as_bytes())),
        "the group's id is derived from its root, not borrowed from a member"
    );

    for id in [11, 22, 33] {
        assert_eq!(
            folded.group_of(ProjectId(id)),
            Some(0),
            "project {id} names /src/vitrum and must land in its group"
        );
    }
    assert_eq!(folded.group_of(ProjectId(44)), Some(1));
    assert_eq!(
        folded.group_of(ProjectId(99)),
        None,
        "an id the daemon never listed has no group, and its rows must not be dropped silently"
    );
}

/// The group's id must be a function of the directory alone, so two
/// windows, a restart, and a client that minted its own id all agree.
#[test]
fn a_projects_group_id_survives_being_listed_under_different_ids() {
    let one = [project(5, "/src/vitrum")];
    let two = [project(9_999, "/src/vitrum/")];
    let elsewhere = [project(5, "/src/other")];
    assert_eq!(
        coalesce_projects(&one).groups()[0].id,
        coalesce_projects(&two).groups()[0].id
    );
    assert_ne!(
        coalesce_projects(&one).groups()[0].id,
        coalesce_projects(&elsewhere).groups()[0].id
    );
}

/// Sixty unlabelled sessions must produce sixty different rows. The daemon
/// names one after its command, so sixty real sessions gave fifty-seven
/// rows reading `bash` and the only way to find one was to open all of
/// them.
#[test]
fn sixty_unlabelled_sessions_produce_sixty_distinct_titles() {
    let mut seen = std::collections::BTreeSet::new();
    for id in 1..=60u64 {
        let mut s = info(id);
        s.command = "/bin/bash".to_string();
        s.title = "bash".to_string();
        let title = row_title(&s).into_owned();
        assert!(title.starts_with("bash"), "got {title:?}");
        assert!(seen.insert(title), "two rows share a title");
    }
    assert_eq!(seen.len(), 60);
    assert!(seen.contains("bash #1"));
    assert!(seen.contains("bash #60"));
}

/// The operator's label wins, and is passed through byte for byte. A
/// disambiguator glued onto a name someone chose is the product editing
/// their words.
#[test]
fn an_operator_label_is_never_decorated() {
    let mut s = info(7);
    s.command = "/bin/bash".to_string();
    s.title = "review auth".to_string();
    assert_eq!(row_title(&s), "review auth");
    assert!(
        matches!(row_title(&s), Cow::Borrowed(_)),
        "a chosen title must not allocate on every paint"
    );

    // A path-shaped command still defaults to its file name, so the
    // generated case has to be recognised through the path.
    s.title = "bash".to_string();
    assert_eq!(row_title(&s), "bash #7");
    s.command = "bash".to_string();
    assert_eq!(row_title(&s), "bash #7");
}

/// The current bucket comes from focus, then from the last tab touched,
/// and from nothing else. Choosing it by activity would move the section
/// while the operator is reading it.
#[test]
fn the_current_session_is_focus_then_recency() {
    assert_eq!(
        current_session(Some(SessionId(4)), &[SessionId(1), SessionId(2)]),
        Some(SessionId(4)),
        "focus outranks recency"
    );
    assert_eq!(
        current_session(None, &[SessionId(1), SessionId(2)]),
        Some(SessionId(2)),
        "the mru is oldest first, so the last entry is the most recent"
    );
    assert_eq!(current_session(None, &[]), None);
}

/// Pinning must move ONE bucket and leave every other relative position
/// exactly as it was. A sort would reorder the tail as a side effect,
/// which is a row moving under the cursor for no reason the operator can
/// see.
#[test]
fn pinning_rotates_and_never_reorders_the_rest() {
    let mut buckets = vec!["a", "b", "c", "d"];
    assert!(pin_current(&mut buckets, |b| *b == "c"));
    assert_eq!(buckets, vec!["c", "a", "b", "d"]);

    assert!(
        !pin_current(&mut buckets, |b| *b == "c"),
        "already at the front is not a move"
    );
    assert_eq!(buckets, vec!["c", "a", "b", "d"]);

    assert!(
        !pin_current(&mut buckets, |b| *b == "zzz"),
        "nothing current is not a move"
    );
    assert_eq!(buckets, vec!["c", "a", "b", "d"]);
}
