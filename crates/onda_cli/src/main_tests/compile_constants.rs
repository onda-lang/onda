use super::*;

const COMMANDS: &[&[&str]] = &[
    &["compile", "x.onda"],
    &["run", "x.onda"],
    &["run"],
    &["run", "play", "x.onda"],
    &["run", "render", "x.onda"],
    &["bench", "x.onda"],
    &["daemon", "diagnose", "x.onda"],
];

#[test]
fn compiling_commands_accept_both_constant_option_forms() {
    for command in COMMANDS {
        let parsed = parse_args(
            ["onda"]
                .into_iter()
                .chain(command.iter().copied())
                .chain(["--const", "Size=8", "--const=Window=[0.0, 0.5, 1.0]"])
                .map(str::to_owned),
        )
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
        let overrides = match parsed {
            Command::Compile {
                const_overrides, ..
            }
            | Command::Run(RunCommand::Window {
                const_overrides, ..
            })
            | Command::Run(RunCommand::Play {
                const_overrides, ..
            })
            | Command::Run(RunCommand::Render {
                const_overrides, ..
            })
            | Command::Daemon(DaemonCommand::Diagnose {
                const_overrides, ..
            }) => const_overrides,
            Command::Bench(options) => options.const_overrides,
            _ => panic!("expected a compiling command"),
        };
        assert_eq!(
            overrides,
            vec![
                ("Size".to_owned(), "8".to_owned()),
                ("Window".to_owned(), "[0.0, 0.5, 1.0]".to_owned()),
            ],
            "{command:?}",
        );
    }
}

#[test]
fn compiling_commands_reject_missing_malformed_and_duplicate_constants() {
    for command in COMMANDS {
        for (options, expected) in [
            (vec!["--const"], "--const requires a name=value pair"),
            (vec!["--const=Size"], "expected name=value"),
            (vec!["--const", "=8"], "non-empty name"),
            (vec!["--const=Size= "], "requires a value"),
            (
                vec!["--const", "Size=4", "--const=Size=8"],
                "configuration constant 'Size' is specified more than once",
            ),
        ] {
            let error = parse_args(
                ["onda"]
                    .into_iter()
                    .chain(command.iter().copied())
                    .chain(options.iter().copied())
                    .map(str::to_owned),
            )
            .err()
            .expect("invalid constants should fail argument parsing");
            assert!(error.contains(expected), "{command:?} {options:?}: {error}");
        }
    }
}
