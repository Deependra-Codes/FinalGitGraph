use crate::run;

/// The steps `scripts/quality-gate.ps1` ran, in the same order; `--fast` skips tests.
pub fn steps(fast: bool) -> Vec<&'static [&'static str]> {
    let mut steps: Vec<&'static [&'static str]> = vec![
        &["fmt", "--all", "--", "--check"],
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        &["check", "--workspace", "--locked"],
    ];
    if !fast {
        steps.push(&["test", "--workspace", "--locked"]);
    }
    steps
}

/// Guardrails first (what `safe:fast` and `safe:full` added), then the cargo steps.
pub fn run(fast: bool) -> Result<(), String> {
    crate::guardrails::run(&crate::paths::repo_root())?;
    run_steps(&steps(fast), |args| run::status(run::cargo().args(args)))?;
    println!("\nQuality gate passed.");
    Ok(())
}

fn run_steps(
    steps: &[&[&str]],
    mut runner: impl FnMut(&[&str]) -> Result<(), String>,
) -> Result<(), String> {
    for step in steps {
        println!("\n> cargo {}", step.join(" "));
        runner(step)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{run_steps, steps};

    #[test]
    fn full_gate_runs_quality_gate_ps1_steps_in_order() {
        assert_eq!(
            steps(false),
            vec![
                &["fmt", "--all", "--", "--check"][..],
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings"
                ][..],
                &["check", "--workspace", "--locked"][..],
                &["test", "--workspace", "--locked"][..],
            ]
        );
    }

    #[test]
    fn fast_gate_skips_tests() {
        assert!(steps(true).iter().all(|step| step[0] != "test"));
        assert_eq!(steps(true).len(), 3);
    }

    #[test]
    fn gate_stops_at_the_first_failing_step() {
        let mut ran = Vec::new();
        let result = run_steps(&steps(false), |step| {
            ran.push(step[0].to_string());
            if step[0] == "clippy" {
                Err("clippy failed".to_string())
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err("clippy failed".to_string()));
        assert_eq!(ran, ["fmt", "clippy"]);
    }
}
