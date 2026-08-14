// Markdown rendering: render dictionary entries as the Markdown shown in hover.
//
// The word's main link jumps to the configured platform (see config::Settings::platform_url).

use crate::config::Settings;
use crate::dict::DictEntry;
use crate::reverse_query::ReverseResult;

/// The word's main link jumps to the default platform. word is the display word (from the query key, i.e. lowercased).
pub fn entry_to_markdown(word: &str, entry: &DictEntry, settings: &Settings) -> String {
    let url = settings.platform_url(word);
    let phonetic = if entry.phonetic.is_empty() {
        String::new()
    } else {
        format!(" _/{}/_", entry.phonetic)
    };
    // Hard line break (two trailing spaces) so the translation starts on a new
    // line even in markdown renderers that collapse a single "\n" into a space.
    let translation = entry.translation.replace("\\n", "  \n");
    format!("[{}]({}){}:  \n{}", word, url, phonetic, translation)
}

/// Generate the Markdown for one Chinese reverse-query result (ReverseResult).
/// ReverseResult has the same fields as DictEntry (word/translation/phonetic),
/// so the same rendering logic is reused.
pub fn reverse_result_to_markdown(r: &ReverseResult, settings: &Settings) -> String {
    let url = settings.platform_url(&r.word);
    let phonetic = if r.phonetic.is_empty() {
        String::new()
    } else {
        format!(" _/{}/_", r.phonetic)
    };
    let translation = r.translation.replace("\\n", "  \n");
    format!("[{}]({}){}:  \n{}", r.word, url, phonetic, translation)
}
