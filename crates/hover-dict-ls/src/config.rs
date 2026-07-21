// 用户配置（来自 Zed settings.json 的 lsp.hover-dict.initialization_options）
//
// 注意：语言级启用/禁用由 Zed 原生的 `languages.<Lang>.language_servers`
// 控制，本扩展不再重复实现黑白名单。

use serde::Deserialize;

/// 翻译平台 URL 模板：{word} 为占位符
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
    /// 中译英最多返回候选数
    #[serde(rename = "hover_dict.chinese_to_english_max_results")]
    pub chinese_to_english_max_results: usize,
    /// 单词/结果跳转的默认平台：google/baidu/deepl/bing/yandex/custom
    #[serde(rename = "hover_dict.default_translate_platform")]
    pub default_translate_platform: String,
    /// default_translate_platform=custom 时的 URL 模板，{word} 占位符
    #[serde(rename = "hover_dict.custom_translate_url")]
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

    /// 根据默认平台与自定义 URL 生成某个单词的跳转链接
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

/// 极简 URL encode（仅编码空格，英文单词场景足够）
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
