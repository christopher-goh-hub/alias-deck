//! Whether installing needs a person, and what to ask. One function over one table.

use alias_deck_core::{Finding, Rule, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    /// `[y/N]`.
    YesNo,
    /// Type the pack name, so a Danger finding is never approved by a reflexive `y`.
    TypeName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Ask(Confirm),
    Proceed,
    /// `--yes` was given, but these Danger rules were not named in `--allow`.
    Refuse {
        unallowed: Vec<Rule>,
    },
}

/// | Highest severity | Interactive        | `--yes`                                      |
/// |------------------|--------------------|----------------------------------------------|
/// | none, Info, Warn | ask `[y/N]`        | proceed                                      |
/// | Danger           | type the pack name | refuse unless each Danger rule is in `allow` |
pub fn decide(findings: &[Finding], yes: bool, allow: &[Rule]) -> Decision {
    let danger = findings.iter().any(|f| f.severity == Severity::Danger);
    match (danger, yes) {
        (false, false) => Decision::Ask(Confirm::YesNo),
        (true, false) => Decision::Ask(Confirm::TypeName),
        (false, true) => Decision::Proceed,
        (true, true) => {
            let mut unallowed: Vec<Rule> = findings
                .iter()
                .filter(|f| f.severity == Severity::Danger && !allow.contains(&f.rule))
                .map(|f| f.rule)
                .collect();
            unallowed.sort();
            unallowed.dedup();
            if unallowed.is_empty() {
                Decision::Proceed
            } else {
                Decision::Refuse { unallowed }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alias_deck_core::{EntryName, EntryRef, EnvName, Slug};

    fn finding(rule: Rule) -> Finding {
        let entry = EntryRef {
            pack: Slug::new("p").unwrap(),
            group: Slug::new("g").unwrap(),
            name: EntryName::Env(EnvName::new("X").unwrap()),
        };
        Finding { rule, severity: rule.severity(), entry, message: String::new() }
    }

    #[test]
    fn the_policy_table() {
        let info = [finding(Rule::Secret)];
        let warn = [finding(Rule::Secret), finding(Rule::EnvHistory)];
        let danger = [finding(Rule::EnvLoaderHijack), finding(Rule::PathUnsafeDir), finding(Rule::EnvHistory)];
        assert_eq!(decide(&[], false, &[]), Decision::Ask(Confirm::YesNo));
        assert_eq!(decide(&info, false, &[]), Decision::Ask(Confirm::YesNo));
        assert_eq!(decide(&warn, false, &[]), Decision::Ask(Confirm::YesNo));
        assert_eq!(decide(&danger, false, &[Rule::EnvLoaderHijack]), Decision::Ask(Confirm::TypeName));
        assert_eq!(decide(&warn, true, &[]), Decision::Proceed);
        assert_eq!(
            decide(&danger, true, &[Rule::EnvLoaderHijack]),
            Decision::Refuse { unallowed: vec![Rule::PathUnsafeDir] }
        );
        assert_eq!(decide(&danger, true, &[Rule::PathUnsafeDir, Rule::EnvLoaderHijack]), Decision::Proceed);
    }
}
