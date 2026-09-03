use std::time::Duration;

/// The small protocol seam shared by ELM dialect users and transport
/// backends.  The backend owns the actual byte exchange; this module owns
/// only ELM command sequencing and response validation.
pub(crate) trait ElmExchange {
    async fn exchange(
        &mut self,
        command: &str,
        command_timeout: Duration,
    ) -> Result<String, String>;
}

/// Verify the generic ELM327 identity before any dialect-specific setup.
pub(crate) async fn verify_elm327<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
    let response = exchange.exchange("ATI\r", Duration::from_secs(3)).await?;
    require_response(
        &response,
        "ELM327",
        false,
        "ATI did not identify an ELM327 adapter",
    )
}

/// Complete generic ELM initialization after a backend has performed its
/// adapter-specific identity check.  Carly calls this after `AT@1`, keeping
/// the established wire order unchanged.
pub(crate) async fn initialize_elm<E>(exchange: &mut E) -> Result<(), String>
where
    E: ElmExchange,
{
    let reset = exchange.exchange("ATZ\r", Duration::from_secs(3)).await?;
    require_response(
        &reset,
        "ELM327",
        false,
        "ATZ did not reset an ELM327 adapter",
    )?;
    // Keep separators and adapter headers so responder identity survives the
    // ELM normalization boundary. No identity is synthesized when absent.
    for command in ["ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r"] {
        let response = exchange.exchange(command, Duration::from_secs(3)).await?;
        require_response(&response, "OK", true, &format!("{} failed", command.trim()))?;
    }
    Ok(())
}

pub(crate) fn require_response(
    response: &str,
    expected: &str,
    exact: bool,
    error: &str,
) -> Result<(), String> {
    let upper = response.to_ascii_uppercase();
    if upper.split(['\r', '\n']).any(|line| {
        let line = line.trim().trim_end_matches('>').trim();
        line == "?"
            || ["NO DATA", "STOPPED", "UNABLE TO CONNECT", "ERROR"]
                .iter()
                .any(|status| line.contains(status))
    }) {
        return Err(format!("{error}: {response:?}"));
    }
    upper
        .split(['\r', '\n'])
        .map(|line| line.trim().trim_end_matches('>').trim())
        .any(|line| {
            if exact {
                line == expected
            } else {
                line.starts_with(expected)
            }
        })
        .then_some(())
        .ok_or_else(|| format!("{error}: {response:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedExchange {
        responses: VecDeque<String>,
        commands: Vec<String>,
    }

    impl ScriptedExchange {
        fn new(responses: impl IntoIterator<Item = &'static str>) -> Self {
            Self {
                responses: responses.into_iter().map(str::to_owned).collect(),
                commands: Vec::new(),
            }
        }
    }

    impl ElmExchange for ScriptedExchange {
        async fn exchange(
            &mut self,
            command: &str,
            _command_timeout: Duration,
        ) -> Result<String, String> {
            self.commands.push(command.to_owned());
            self.responses
                .pop_front()
                .ok_or_else(|| "script ended before adapter response".to_string())
        }
    }

    #[tokio::test]
    async fn generic_initialization_leaves_carly_identity_to_the_backend() {
        let mut exchange = ScriptedExchange::new([
            "ELM327 v1.4 v100\r>",
            "ELM327 v1.4 v100\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
            "OK\r>",
        ]);

        verify_elm327(&mut exchange).await.unwrap();
        initialize_elm(&mut exchange).await.unwrap();

        assert_eq!(
            exchange.commands,
            ["ATI\r", "ATZ\r", "ATE0\r", "ATL0\r", "ATS1\r", "ATH1\r", "ATSP0\r"]
        );
    }
}
