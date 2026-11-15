//! Conservative correction of final accents lost by the built-in OCR reader.

use std::collections::HashMap;
use std::sync::OnceLock;

// Only explicit spellings: no suffix rules or grammatical guesses. Omit short
// ambiguous words and English collisions (including eta, pie, faro and casino).
const WORDS: &[&str] = &[
    "perché",
    "poiché",
    "affinché",
    "benché",
    "finché",
    "giacché",
    "nonché",
    "purché",
    "sicché",
    "cosicché",
    "dopodiché",
    "granché",
    "alcunché",
    "pressoché",
    "macché",
    "cioè",
    "più",
    "già",
    "giù",
    "così",
    "però",
    "lassù",
    "quassù",
    "laggiù",
    "quaggiù",
    "chissà",
    "può",
    "lunedì",
    "martedì",
    "mercoledì",
    "giovedì",
    "venerdì",
    "caffè",
    "ventitré",
    "trentatré",
    "quarantatré",
    "cinquantatré",
    "sessantatré",
    "settantatré",
    "ottantatré",
    "novantatré",
    "città",
    "università",
    "società",
    "libertà",
    "verità",
    "attività",
    "proprietà",
    "velocità",
    "quantità",
    "qualità",
    "possibilità",
    "difficoltà",
    "facoltà",
    "comunità",
    "identità",
    "realtà",
    "umanità",
    "dignità",
    "responsabilità",
    "disponibilità",
    "probabilità",
    "utilità",
    "priorità",
    "autorità",
    "sanità",
    "pubblicità",
    "novità",
    "curiosità",
    "personalità",
    "nazionalità",
    "località",
    "modalità",
    "finalità",
    "totalità",
    "normalità",
    "anormalità",
    "formalità",
    "informalità",
    "legalità",
    "illegalità",
    "validità",
    "invalidità",
    "intensità",
    "densità",
    "profondità",
    "umidità",
    "elettricità",
    "elasticità",
    "plasticità",
    "rigidità",
    "fluidità",
    "solidità",
    "fragilità",
    "sensibilità",
    "visibilità",
    "invisibilità",
    "leggibilità",
    "illeggibilità",
    "accessibilità",
    "compatibilità",
    "incompatibilità",
    "affidabilità",
    "inaffidabilità",
    "sostenibilità",
    "insostenibilità",
    "produttività",
    "competitività",
    "creatività",
    "effettività",
    "oggettività",
    "soggettività",
    "relatività",
    "continuità",
    "discontinuità",
    "simultaneità",
    "spontaneità",
    "contemporaneità",
    "omogeneità",
    "eterogeneità",
    "idoneità",
    "estraneità",
    "affinità",
    "serenità",
    "severità",
    "sincerità",
    "lealtà",
    "slealtà",
    "fedeltà",
    "infedeltà",
    "bontà",
    "cattività",
    "crudeltà",
    "povertà",
    "sobrietà",
    "notorietà",
    "varietà",
    "serietà",
    "solidarietà",
    "precarietà",
    "contrarietà",
    "arbitrarietà",
    "obbligatorietà",
    "volontarietà",
    "complementarietà",
    "familiarità",
    "regolarità",
    "irregolarità",
    "singolarità",
    "pluralità",
    "dualità",
    "parità",
    "disparità",
    "maggiorità",
    "minorità",
    "superiorità",
    "inferiorità",
    "anteriorità",
    "posteriorità",
    "interiorità",
    "esteriorità",
    "maturità",
    "immaturità",
    "oscurità",
    "celerità",
    "rapidità",
    "brevità",
    "longevità",
    "fertilità",
    "sterilità",
    "mortalità",
    "immortalità",
    "vitalità",
    "natalità",
    "ospitalità",
    "ostilità",
    "tranquillità",
    "agilità",
    "disabilità",
    "vulnerabilità",
    "variabilità",
    "invariabilità",
    "durabilità",
    "portabilità",
    "tracciabilità",
    "ripetibilità",
    "riproducibilità",
    "reversibilità",
    "irreversibilità",
    "divisibilità",
    "indivisibilità",
    "solubilità",
    "insolubilità",
    "permeabilità",
    "impermeabilità",
    "conducibilità",
    "conduttività",
    "reattività",
    "radioattività",
    "tossicità",
    "specificità",
    "periodicità",
    "complessità",
    "semplicità",
    "molteplicità",
    "unicità",
    "autenticità",
    "praticità",
    "economicità",
    "elettronegatività",
    "polarità",
    "neutralità",
    "centralità",
    "lateralità",
    "verticalità",
    "orizzontalità",
    "linearità",
    "nonlinearità",
    "proporzionalità",
    "funzionalità",
    "razionalità",
    "irrazionalità",
    "emotività",
    "affettività",
    "aggressività",
    "passività",
    "intenzionalità",
    "spiritualità",
    "religiosità",
    "generosità",
    "avidità",
    "vanità",
    "banalità",
    "originalità",
    "rarità",
    "antichità",
    "modernità",
    "civiltà",
    "entità",
    "integrità",
    "immunità",
    "impunità",
    "fatalità",
    "casualità",
    "causalità",
];

fn dictionary() -> &'static HashMap<String, char> {
    static DICTIONARY: OnceLock<HashMap<String, char>> = OnceLock::new();
    DICTIONARY.get_or_init(|| {
        WORDS
            .iter()
            .map(|word| {
                let accent = word.chars().last().unwrap();
                let vowel = match accent {
                    'à' => 'a',
                    'è' | 'é' => 'e',
                    'ì' => 'i',
                    'ò' => 'o',
                    'ù' => 'u',
                    _ => panic!("dictionary entries must end in an accented vowel"),
                };
                let mut key = word[..word.len() - accent.len_utf8()].to_owned();
                key.push(vowel);
                (key, accent)
            })
            .collect()
    })
}

/// Restore only listed Italian words, retaining the original letter casing.
/// Whitespace-delimited URL/email spans are preserved in their entirety.
pub fn restore_italian_accents(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for span in text.split_inclusive(char::is_whitespace) {
        let domain = span
            .as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_alphanumeric() && w[1] == b'.' && w[2].is_ascii_alphanumeric());
        if span.contains('@') || span.contains("://") || span.contains('/') || domain {
            output.push_str(span);
            continue;
        }
        let mut start = 0;
        for (end, ch) in span.char_indices() {
            // Digits, underscores and combining marks stay attached so a
            // fragment of an identifier or an accented word cannot match.
            if ch.is_alphanumeric()
                || ch == '_'
                || ch == '?'
                || ('\u{0300}'..='\u{036f}').contains(&ch)
            {
                continue;
            }
            restore_word(&span[start..end], &mut output);
            output.push(ch);
            start = end + ch.len_utf8();
        }
        restore_word(&span[start..], &mut output);
    }
    output
}

fn restore_word(word: &str, output: &mut String) {
    // A normal word followed by '?' is a question, not a missing vowel.
    let (candidate, punctuation) = if let Some(prefix) = word.strip_suffix('?') {
        if dictionary().contains_key(&prefix.to_ascii_lowercase()) {
            (prefix, "?")
        } else {
            (word, "")
        }
    } else {
        (word, "")
    };
    let mut key = candidate.to_ascii_lowercase();
    let accent = if key.ends_with('?') && key.bytes().filter(|b| *b == b'?').count() == 1 {
        key.pop();
        // Explicitly search the five possible final vowels. Refuse collisions.
        let mut found = None;
        for vowel in ['a', 'e', 'i', 'o', 'u'] {
            key.push(vowel);
            if let Some(&accent) = dictionary().get(&key) {
                if found.is_some() {
                    output.push_str(word);
                    return;
                }
                found = Some(accent);
            }
            key.pop();
        }
        found
    } else {
        dictionary().get(&key).copied()
    };
    if let Some(accent) = accent {
        let (last_index, last) = candidate.char_indices().last().unwrap();
        output.push_str(&candidate[..last_index]);
        let uppercase = last.is_uppercase()
            || (last == '?'
                && candidate.chars().any(char::is_uppercase)
                && !candidate.chars().any(char::is_lowercase));
        if uppercase {
            output.extend(accent.to_uppercase());
        } else {
            output.push(accent);
        }
        output.push_str(punctuation);
    } else {
        output.push_str(word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_words_and_case() {
        assert!((150..=400).contains(&WORDS.len()));
        assert_eq!(dictionary().len(), WORDS.len());
        for word in WORDS {
            let accent = word.chars().last().unwrap();
            let plain = dictionary()
                .iter()
                .find(|(key, value)| {
                    **value == accent
                        && key[..key.len() - 1] == word[..word.len() - accent.len_utf8()]
                })
                .unwrap()
                .0
                .clone();
            assert_eq!(restore_italian_accents(&plain), *word);
            assert_eq!(
                restore_italian_accents(&plain.to_uppercase()),
                word.to_uppercase()
            );
            let title = format!("{}{}", plain[..1].to_uppercase(), &plain[1..]);
            let expected = format!("{}{}", word[..1].to_uppercase(), &word[1..]);
            assert_eq!(restore_italian_accents(&title), expected);
        }
    }

    #[test]
    fn missing_final_vowel() {
        assert_eq!(
            restore_italian_accents("citt? CITT? Citt? perch? caff? pi?"),
            "città CITTÀ Città perché caffè più"
        );
        assert_eq!(
            restore_italian_accents("ci?ta citt?? ?citta per?he"),
            "ci?ta citt?? ?citta per?he"
        );
    }

    #[test]
    fn punctuation_and_whole_words() {
        assert_eq!(
            restore_italian_accents("(perche), «citta»; l'universita! piu?\n[citt?]"),
            "(perché), «città»; l'università! più?\n[città]"
        );
        let text = "supercitta cittadino 2citta citta2 ²citta citta_ citta\u{0301}";
        assert_eq!(restore_italian_accents(text), text);
    }

    #[test]
    fn ambiguous_and_english_words_unchanged() {
        let text = "e a da la li se si ne te di papa pesca ancora eta pie faro casino meta vita abilita gravita capacita necessita onesta disonesta stabilita mobilita eredita calamita";
        assert_eq!(restore_italian_accents(text), text);
        let english = "The society has a university and a cafe. Can I have more tea, please?";
        assert_eq!(restore_italian_accents(english), english);
    }

    #[test]
    fn urls_and_emails_unchanged() {
        let text = "https://example.com/perche?citta=piu www.citta.it citta.it/perche perche@example.it mailto:citta@example.it https://citta/perche (https://host/citt?)";
        assert_eq!(restore_italian_accents(text), text);
    }

    #[test]
    fn already_accented_and_idempotent() {
        let text = "Perché la città è già così: caffè, università, più libertà.";
        assert_eq!(restore_italian_accents(text), text);
        let restored = restore_italian_accents("Perche citta citt? CITT? piu");
        assert_eq!(restore_italian_accents(&restored), restored);
        assert_eq!(restore_italian_accents(""), "");
    }
}
