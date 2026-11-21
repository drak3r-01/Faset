//! stdin console commands.

/// A parsed console line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print server state.
    Status,
    /// Remove a player by id.
    Kick(u16),
    /// Change map.
    Map(String),
    /// Broadcast a message.
    Say(String),
    /// Stop the server.
    Quit,
}

/// Parses one line; `Ok(None)` for a blank line.
///
/// # Errors
/// A message for the operator when the line is not a valid command.
pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let line = line.trim();
    let (cmd, arg) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let arg = arg.trim();
    let need = |what: &str| {
        if arg.is_empty() {
            Err(format!("usage: {cmd} <{what}>"))
        } else {
            Ok(arg.to_owned())
        }
    };
    Ok(Some(match cmd {
        "" => return Ok(None),
        "status" => Command::Status,
        "quit" | "exit" | "stop" => Command::Quit,
        "kick" => Command::Kick(
            need("player")?
                .trim_start_matches('#')
                .parse()
                .map_err(|_| "player must be a numeric id".to_owned())?,
        ),
        "map" => Command::Map(need("name")?),
        "say" => Command::Say(need("text")?),
        other => {
            return Err(format!(
                "unknown command `{other}` (status, kick, map, say, quit)"
            ))
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse() {
        assert_eq!(parse("status"), Ok(Some(Command::Status)));
        assert_eq!(parse(" kick #3 "), Ok(Some(Command::Kick(3))));
        assert_eq!(
            parse("say hello  world"),
            Ok(Some(Command::Say("hello  world".into())))
        );
        assert_eq!(parse("map arena"), Ok(Some(Command::Map("arena".into()))));
        assert_eq!(parse("quit"), Ok(Some(Command::Quit)));
        assert_eq!(parse(""), Ok(None));
        assert!(parse("kick").is_err());
        assert!(parse("kick bob").is_err());
        assert!(parse("dance").is_err());
    }
}
