// User config (from Zed settings.json's lsp.translate-dict.initialization_options)
//
// Note: language-level enable/disable is controlled by Zed's native
// `languages.<Lang>.language_servers`; this extension does not reimplement allow/deny.

use serde::Deserialize;

/// Translation platform URL templates: {word} is the placeholder
pub const PLATFORM_URLS: &[(&str, &str)] = &[
    ("google", "https://translate.google.com/?text={word}"),
    ("baidu", "https://fanyi.baidu.com/#en/zh/{word}"),
    ("deepl", "https://www.deepl.com/translator#en/zh/{word}"),
    ("bing", "https://www.bing.com/translator/?text={word}"),
    ("yandex", "https://translate.yandex.net/?text={word}"),
];

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// Max candidates returned for Chinese-to-English
    #[serde(rename = "translate_dict_lsp.chinese_to_english_max_results")]
    pub chinese_to_english_max_results: usize,
    /// Default platform for word/result links: google/baidu/deepl/bing/yandex/custom
    #[serde(rename = "translate_dict_lsp.default_translate_platform")]
    pub default_translate_platform: String,
    /// URL template when default_translate_platform=custom; {word} placeholder
    #[serde(rename = "translate_dict_lsp.custom_translate_url")]
    pub custom_translate_url: String,
}

impl Settings {
    pub fn max_results(&self) -> usize {
        if self.chinese_to_english_max_results == 0 {
            10
        } else {
            self.chinese_to_english_max_results.min(50)
        }
    }

    /// Build the jump link for a word from the default platform and custom URL
    pub fn platform_url(&self, word: &str) -> String {
        let encoded = urlencode(word);
        let template: &str = if self.default_translate_platform == "custom"
            && !self.custom_translate_url.is_empty()
        {
            &self.custom_translate_url
        } else {
            PLATFORM_URLS
                .iter()
                .find(|(name, _)| *name == self.default_translate_platform)
                .map(|(_, t)| *t)
                .unwrap_or(PLATFORM_URLS[0].1)
        };
        template.replace("{word}", &encoded)
    }
}

/// Minimal URL encoding (only encodes spaces; sufficient for English words)
pub fn urlencode(s: &str) -> String {
    s.replace(' ', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_max_results() {
        assert_eq!(Settings::default().max_results(), 10);
    }

    #[test]
    fn test_platform_url_google() {
        let s = Settings {
            default_translate_platform: "google".to_string(),
            ..Default::default()
        };
        assert!(s.platform_url("hello").contains("translate.google.com"));
    }

    #[test]
    fn test_platform_url_custom() {
        let s = Settings {
            default_translate_platform: "custom".to_string(),
            custom_translate_url: "https://example.com/{word}".to_string(),
            ..Default::default()
        };
        assert_eq!(s.platform_url("hi"), "https://example.com/hi");
    }

    #[test]
    fn test_platform_url_unknown_falls_back_to_google() {
        let s = Settings {
            default_translate_platform: "nope".to_string(),
            ..Default::default()
        };
        assert!(s.platform_url("x").contains("translate.google.com"));
    }
}
