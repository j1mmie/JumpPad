use super::*;

fn parse(args: &[&str]) -> Invocation {
    parse_args(args.iter().map(OsString::from))
}

fn paths(args: &[&str]) -> Vec<PathBuf> {
    match parse(args) {
        Invocation::Open(paths) => paths,
        other => panic!("expected paths, got {other:?}"),
    }
}

#[test]
fn no_arguments_opens_nothing() {
    assert!(paths(&[]).is_empty());
}

#[test]
fn bare_arguments_are_paths_in_order() {
    assert_eq!(
        paths(&["a.txt", "../b.md"]),
        vec![PathBuf::from("a.txt"), PathBuf::from("../b.md")]
    );
}

#[test]
fn help_and_version_win_wherever_they_appear() {
    assert_eq!(parse(&["-h"]), Invocation::Help);
    assert_eq!(parse(&["--help"]), Invocation::Help);
    assert_eq!(parse(&["a.txt", "--help"]), Invocation::Help);
    assert_eq!(parse(&["-V"]), Invocation::Version);
    assert_eq!(parse(&["--version"]), Invocation::Version);
    assert_eq!(parse(&["a.txt", "--version"]), Invocation::Version);
}

#[test]
fn an_unrecognized_flag_is_just_a_filename() {
    // No flag surface beyond the two above - `-x` becomes a path, and
    // fails later as a missing file rather than as a usage error.
    assert_eq!(paths(&["-x"]), vec![PathBuf::from("-x")]);
}

#[test]
fn program_name_falls_back_when_argv0_is_missing_or_odd() {
    assert_eq!(
        program_name(Some(&OsString::from("/usr/bin/jumppad"))),
        "jumppad"
    );
    assert_eq!(
        program_name(Some(&OsString::from("jumppad-gpu"))),
        "jumppad-gpu"
    );
    assert_eq!(program_name(None), "jumppad");
}
