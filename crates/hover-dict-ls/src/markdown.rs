// Markdown 渲染：把词典词条渲染成 hover 展示的 Markdown 文本。
//
// 单词主链接跳转到配置的平台（见 config::Settings::platform_url）。

use crate::config::Settings;
use crate::dict::DictEntry;
use crate::reverse_query::ReverseResult;

/// 生成一条词条的 Markdown（对齐 translate-dict 的 convert.ts::genMarkdown）
/// 单词主链接跳转到默认平台。word 为展示用单词（取自查询键，即小写词）。
pub fn entry_to_markdown(word: &str, entry: &DictEntry, settings: &Settings) -> String {
    let url = settings.platform_url(word);
    let phonetic = if entry.phonetic.is_empty() {
        String::new()
    } else {
        format!(" _/{}/_", entry.phonetic)
    };
    let translation = entry.translation.replace("\\n", "  \n");
    format!("- [{}]({}) {}:\n{}", word, url, phonetic, translation)
}

/// 生成一条中文反查结果（ReverseResult）的 Markdown。
/// ReverseResult 与 DictEntry 字段相同（word/translation/phonetic），
/// 仅多路复用同一套渲染逻辑。
pub fn reverse_result_to_markdown(r: &ReverseResult, settings: &Settings) -> String {
    let url = settings.platform_url(&r.word);
    let phonetic = if r.phonetic.is_empty() {
        String::new()
    } else {
        format!(" _/{}/_", r.phonetic)
    };
    let translation = r.translation.replace("\\n", "  \n");
    format!("- [{}]({}) {}:\n{}", r.word, url, phonetic, translation)
}
