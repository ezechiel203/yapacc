//! Interactive confirmation gate (localized Y/N).
//!
//! Semantics for mutating commands:
//!   --yes            proceed (no prompt)
//!   --noconfirm      proceed (no prompt)
//!   --script         NEVER prompt and NEVER auto-proceed: fail fast
//!   otherwise        prompt [y/N] (letters localized from LC_ALL/LC_MESSAGES/LANG)
//!                    on a TTY; non-TTY stdin fails fast (piped input cannot confirm)
//!
//! Empty answer or any localized/English "no" aborts; localized or English
//! "yes" proceeds. Default is NO (safe side, pacman-style).

use std::io::{IsTerminal, Write};

/// The last line of every dry run, on every command.
pub const DRY_RUN_FOOTER: &str =
    "DRY-RUN: nothing was changed. Run again without --dryrun to apply.";

/// The last line of a `plan` command, which never applies anything.
pub const PLAN_ONLY_FOOTER: &str = "PLAN ONLY: nothing was changed.";

#[derive(Clone, Copy, Debug)]
pub struct ConfirmLocale {
    /// language code
    pub lang: &'static str,
    /// single letter meaning yes (lowercase), e.g. "y", "o", "j"
    pub yes_letter: &'static str,
    /// single letter meaning no (uppercase in the prompt: default answer)
    pub no_letter: &'static str,
    /// full words accepted for yes (lowercase)
    pub yes_words: &'static [&'static str],
    /// full words accepted for no (lowercase; informational — anything not
    /// yes, including empty input, means no)
    #[allow(dead_code)]
    pub no_words: &'static [&'static str],
}

const EN: ConfirmLocale = ConfirmLocale {
    lang: "en",
    yes_letter: "y",
    no_letter: "N",
    yes_words: &["yes"],
    no_words: &["no"],
};

const LOCALES: &[ConfirmLocale] = &[
    ConfirmLocale {
        lang: "en",
        yes_letter: "y",
        no_letter: "N",
        yes_words: &["yes"],
        no_words: &["no"],
    },
    ConfirmLocale {
        lang: "fr",
        yes_letter: "o",
        no_letter: "N",
        yes_words: &["oui", "o"],
        no_words: &["non", "n"],
    },
    ConfirmLocale {
        lang: "de",
        yes_letter: "j",
        no_letter: "N",
        yes_words: &["ja", "j"],
        no_words: &["nein", "n"],
    },
    ConfirmLocale {
        lang: "es",
        yes_letter: "s",
        no_letter: "N",
        yes_words: &["si", "sí", "s"],
        no_words: &["no", "n"],
    },
    ConfirmLocale {
        lang: "it",
        yes_letter: "s",
        no_letter: "N",
        yes_words: &["si", "sì", "s"],
        no_words: &["no", "n"],
    },
    ConfirmLocale {
        lang: "pt",
        yes_letter: "s",
        no_letter: "N",
        yes_words: &["sim", "s"],
        no_words: &["nao", "não", "n"],
    },
    ConfirmLocale {
        lang: "nl",
        yes_letter: "j",
        no_letter: "N",
        yes_words: &["ja", "j"],
        no_words: &["nee", "n"],
    },
    ConfirmLocale {
        lang: "sv",
        yes_letter: "j",
        no_letter: "N",
        yes_words: &["ja", "j"],
        no_words: &["nej", "n"],
    },
    ConfirmLocale {
        lang: "da",
        yes_letter: "j",
        no_letter: "N",
        yes_words: &["ja", "j"],
        no_words: &["nej", "n"],
    },
    ConfirmLocale {
        lang: "no",
        yes_letter: "j",
        no_letter: "N",
        yes_words: &["ja", "j"],
        no_words: &["nei", "n"],
    },
    ConfirmLocale {
        lang: "pl",
        yes_letter: "t",
        no_letter: "N",
        yes_words: &["tak", "t"],
        no_words: &["nie", "n"],
    },
    ConfirmLocale {
        lang: "cs",
        yes_letter: "a",
        no_letter: "N",
        yes_words: &["ano", "a"],
        no_words: &["ne", "n"],
    },
    ConfirmLocale {
        lang: "sk",
        yes_letter: "a",
        no_letter: "N",
        yes_words: &["ano", "a"],
        no_words: &["nie", "n"],
    },
    ConfirmLocale {
        lang: "ru",
        yes_letter: "д",
        no_letter: "Н",
        yes_words: &["да", "д"],
        no_words: &["нет", "н"],
    },
    ConfirmLocale {
        lang: "uk",
        yes_letter: "т",
        no_letter: "Н",
        yes_words: &["так", "т"],
        no_words: &["ні", "н"],
    },
    ConfirmLocale {
        lang: "tr",
        yes_letter: "e",
        no_letter: "H",
        yes_words: &["evet", "e"],
        no_words: &["hayir", "hayır", "h"],
    },
    ConfirmLocale {
        lang: "hu",
        yes_letter: "i",
        no_letter: "N",
        yes_words: &["igen", "i"],
        no_words: &["nem", "n"],
    },
    ConfirmLocale {
        lang: "ro",
        yes_letter: "d",
        no_letter: "N",
        yes_words: &["da", "d"],
        no_words: &["nu", "n"],
    },
];

/// Resolve a ConfirmLocale from a language tag ("fr_FR.UTF-8" -> fr table).
pub fn locale_for(lang_tag: &str) -> ConfirmLocale {
    let lang = lang_tag
        .split('.')
        .next()
        .unwrap_or("")
        .split('@')
        .next()
        .unwrap_or("")
        .split('_')
        .next()
        .unwrap_or("")
        .to_lowercase();
    LOCALES
        .iter()
        .find(|l| l.lang == lang)
        .copied()
        .unwrap_or(EN)
}

/// Locale for the current process environment.
pub fn current_locale() -> ConfirmLocale {
    let tag = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    locale_for(&tag)
}

/// Does this answer mean yes? (accepts localized and English forms; case-insensitive)
pub fn answer_is_yes(input: &str, loc: ConfirmLocale) -> bool {
    let a = input.trim().to_lowercase();
    if a.is_empty() {
        return false; // default: NO
    }
    let yes_letter = loc.yes_letter.to_lowercase();
    if a == yes_letter || loc.yes_words.iter().any(|w| *w == a) {
        return true;
    }
    // English fallback so mixed-locale input still works
    a == "y" || a == "yes"
}

/// Render the prompt suffix, e.g. " [o/N] " for French.
pub fn prompt_hint(loc: ConfirmLocale) -> String {
    format!(" [{}/{}]", loc.yes_letter, loc.no_letter)
}

/// The question as it is printed, by the gate and by `confirm_on_terminal`
/// alike: `Proceed [y/N]? `, with a space between the question mark and
/// what is typed. Pure.
pub fn prompt_text(desc: &str, loc: ConfirmLocale) -> String {
    format!("{desc}{}? ", prompt_hint(loc))
}

/// The confirmation gate.
/// Ok(true) = proceed, Ok(false) = user answered no, Err = fail-fast condition.
pub fn confirm_or_proceed(
    desc: &str,
    yes: bool,
    noconfirm: bool,
    script_mode: bool,
) -> Result<bool, String> {
    if yes || noconfirm {
        return Ok(true);
    }
    if script_mode {
        return Err(format!(
            "{desc}: --script never prompts; pass --yes to apply or --dryrun to plan only"
        ));
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{desc}: stdin is not a terminal; pass --yes (or --noconfirm) to apply, or --dryrun to plan only"
        ));
    }
    let loc = current_locale();
    print!("{}", prompt_text(desc, loc));
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => return Ok(false), // EOF: treat as no
        Ok(_) => {}
        Err(e) => return Err(format!("{desc}: failed to read confirmation: {e}")),
    }
    Ok(answer_is_yes(&line, loc))
}

/// Ask on the terminal, whatever `--yes` says. For a question the user must
/// answer themselves. Ok(true) = yes; a run with no terminal is an error,
/// never an assumed yes.
pub fn confirm_on_terminal(desc: &str) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{desc}: this needs your answer at a terminal (--yes, --script and a pipe do not answer it)"
        ));
    }
    let loc = current_locale();
    print!("{}", prompt_text(desc, loc));
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => {
            println!();
            Ok(false)
        }
        Ok(_) => Ok(answer_is_yes(&line, loc)),
        Err(e) => Err(format!("{desc}: failed to read the answer: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_tag_parsing() {
        assert_eq!(locale_for("fr_FR.UTF-8").lang, "fr");
        assert_eq!(locale_for("de_DE.UTF-8").yes_letter, "j");
        assert_eq!(locale_for("en_US").lang, "en");
        assert_eq!(locale_for("ru_RU.UTF-8").yes_letter, "д");
        assert_eq!(locale_for("pt_BR.UTF-8").no_letter, "N");
        assert_eq!(locale_for("unknown_XY").lang, "en"); // fallback
        assert_eq!(locale_for("").lang, "en");
        assert_eq!(locale_for("C").lang, "en");
    }

    #[test]
    fn yes_answers_by_locale() {
        let fr = locale_for("fr_FR");
        assert!(answer_is_yes("o", fr));
        assert!(answer_is_yes("OUI", fr));
        assert!(answer_is_yes("oui", fr));
        assert!(answer_is_yes("y", fr)); // English fallback
        assert!(!answer_is_yes("n", fr));
        assert!(!answer_is_yes("non", fr));
        assert!(!answer_is_yes("", fr)); // default no
        let de = locale_for("de_DE");
        assert!(answer_is_yes("j", de));
        assert!(answer_is_yes("ja", de));
        assert!(!answer_is_yes("n", de));
        let ru = locale_for("ru_RU");
        assert!(answer_is_yes("да", ru));
        assert!(!answer_is_yes("нет", ru));
    }

    #[test]
    fn prompt_hint_shapes() {
        assert_eq!(prompt_hint(locale_for("en_US")), " [y/N]");
        // P9: the answer is typed after a space, at the gate too
        assert_eq!(
            prompt_text("Proceed", locale_for("en_US")),
            "Proceed [y/N]? "
        );
        assert_eq!(
            prompt_text("Proceed", locale_for("fr_FR")),
            "Proceed [o/N]? "
        );
        assert_eq!(prompt_hint(locale_for("fr_FR")), " [o/N]");
        assert_eq!(prompt_hint(locale_for("de_DE")), " [j/N]");
    }

    #[test]
    fn gate_flags() {
        // --yes / --noconfirm proceed without prompting
        assert!(confirm_or_proceed("x", true, false, false).unwrap());
        assert!(confirm_or_proceed("x", false, true, false).unwrap());
        // --script fails fast instead of prompting
        let e = confirm_or_proceed("x", false, false, true).unwrap_err();
        assert!(e.contains("--script"), "got: {e}");
    }
}
