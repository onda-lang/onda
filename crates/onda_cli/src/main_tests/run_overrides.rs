use super::*;

const COMMANDS: &[&[&str]] = &[
    &["run"],
    &["run", "x.onda"],
    &["run", "play", "x.onda"],
    &["run", "render", "x.onda"],
    &["bench", "x.onda"],
];

#[test]
fn execution_commands_share_typed_parameters_and_buffer_overrides() {
    for command in COMMANDS {
        let parsed = parse_args(
            ["onda"]
                .into_iter()
                .chain(command.iter().copied())
                .chain([
                    "--set",
                    "enabled=true",
                    "--set=values[1]=0.25",
                    "--set=wide=9007199254740993",
                    "--buffer",
                    "bank[1]=a path=with equals.ondabuffer",
                    "--buffer=bank[0]=",
                ])
                .map(str::to_owned),
        )
        .unwrap();
        let (params, buffers) = match parsed {
            Command::Run(RunCommand::Window {
                param_sets,
                buffer_bindings,
                ..
            })
            | Command::Run(RunCommand::Play {
                param_sets,
                buffer_bindings,
                ..
            })
            | Command::Run(RunCommand::Render {
                param_sets,
                buffer_bindings,
                ..
            }) => (param_sets, buffer_bindings),
            Command::Bench(options) => (options.param_sets, options.buffer_bindings),
            _ => panic!("expected execution command"),
        };
        assert_eq!(
            params,
            vec![
                ("enabled".into(), onda_daemon::RunParamValue::Bool(true)),
                ("values[1]".into(), onda_daemon::RunParamValue::F64(0.25)),
                (
                    "wide".into(),
                    onda_daemon::RunParamValue::I64(9007199254740993)
                ),
            ]
        );
        assert_eq!(
            buffers,
            vec![
                (
                    "bank[1]".into(),
                    Some(PathBuf::from("a path=with equals.ondabuffer"))
                ),
                ("bank[0]".into(), None),
            ]
        );
    }
}

#[test]
fn execution_commands_reject_duplicate_or_malformed_runtime_overrides() {
    for command in COMMANDS {
        for (flags, expected) in [
            (
                vec!["--set", "gain=1", "--set=gain=2"],
                "parameter 'gain' is specified more than once",
            ),
            (
                vec!["--buffer", "clip=first.wav", "--buffer=clip="],
                "buffer 'clip' is specified more than once",
            ),
            (vec!["--set"], "requires a name=value pair"),
            (vec!["--set=gain"], "expected name=value"),
            (vec!["--set==1"], "non-empty name"),
            (vec!["--set=enabled="], "expected a number or true/false"),
            (vec!["--set=mode=Second"], "expected a number or true/false"),
            (vec!["--set=gains=[1,2]"], "expected a number or true/false"),
            (vec!["--buffer"], "requires a name=value pair"),
            (vec!["--buffer=clip"], "expected name=path"),
            (vec!["--buffer==clip.wav"], "non-empty name"),
        ] {
            let error = parse_args(
                ["onda"]
                    .into_iter()
                    .chain(command.iter().copied())
                    .chain(flags.iter().copied())
                    .map(str::to_owned),
            )
            .err()
            .unwrap();
            assert!(error.contains(expected), "{command:?} {flags:?}: {error}");
        }
    }
    assert!(parse_args(
        [
            "onda",
            "project",
            "out",
            "--from=main.onda",
            "--buffer=clip=a.wav",
            "--buffer=clip=b.wav"
        ]
        .into_iter()
        .map(str::to_owned)
    )
    .err()
    .unwrap()
    .contains("buffer 'clip' is specified more than once"));
}
