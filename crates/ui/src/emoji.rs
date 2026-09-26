//! 名前の中の絵文字のショートコード（O20・A08）。Task・スレッド・端末のタブの名前に `:rocket:` と書くと
//! 🚀 にする（GitHub / Slack と同じ書き方）。表はよく使う物だけを持つ（gitmoji と一般的な物）。
//! 表に無い `:word:` と、時刻のような `12:30:45` はそのまま残す。

/// ショートコード → 絵文字（名前の昇順・二分探索する）。
const SHORTCODES: &[(&str, &str)] = &[
    ("+1", "👍"),
    ("-1", "👎"),
    ("100", "💯"),
    ("adhesive_bandage", "🩹"),
    ("alarm_clock", "⏰"),
    ("alembic", "⚗️"),
    ("ambulance", "🚑"),
    ("arrow_down", "⬇️"),
    ("arrow_up", "⬆️"),
    ("art", "🎨"),
    ("bangbang", "‼️"),
    ("beers", "🍻"),
    ("bell", "🔔"),
    ("bento", "🍱"),
    ("bike", "🚲"),
    ("book", "📖"),
    ("bookmark", "🔖"),
    ("books", "📚"),
    ("boom", "💥"),
    ("brain", "🧠"),
    ("bricks", "🧱"),
    ("bug", "🐛"),
    ("building_construction", "🏗️"),
    ("bulb", "💡"),
    ("busts_in_silhouette", "👥"),
    ("cake", "🍰"),
    ("calendar", "📅"),
    ("camera_flash", "📸"),
    ("card_file_box", "🗃️"),
    ("cat", "🐱"),
    ("chart_with_upwards_trend", "📈"),
    ("children_crossing", "🚸"),
    ("clap", "👏"),
    ("clipboard", "📋"),
    ("cloud", "☁️"),
    ("clown_face", "🤡"),
    ("coffee", "☕"),
    ("coffin", "⚰️"),
    ("computer", "💻"),
    ("construction", "🚧"),
    ("crown", "👑"),
    ("dizzy", "💫"),
    ("dog", "🐶"),
    ("egg", "🥚"),
    ("email", "📧"),
    ("eyes", "👀"),
    ("file_folder", "📁"),
    ("fire", "🔥"),
    ("floppy_disk", "💾"),
    ("gear", "⚙️"),
    ("gem", "💎"),
    ("ghost", "👻"),
    ("gift", "🎁"),
    ("globe_with_meridians", "🌐"),
    ("goal_net", "🥅"),
    ("green_heart", "💚"),
    ("hammer", "🔨"),
    ("heart", "❤️"),
    ("heavy_minus_sign", "➖"),
    ("heavy_plus_sign", "➕"),
    ("hourglass", "⌛"),
    ("inbox_tray", "📥"),
    ("iphone", "📱"),
    ("key", "🔑"),
    ("keyboard", "⌨️"),
    ("label", "🏷️"),
    ("link", "🔗"),
    ("lipstick", "💄"),
    ("lock", "🔒"),
    ("loud_sound", "🔊"),
    ("mag", "🔍"),
    ("mega", "📣"),
    ("memo", "📝"),
    ("microscope", "🔬"),
    ("money_with_wings", "💸"),
    ("monocle_face", "🧐"),
    ("moon", "🌙"),
    ("muscle", "💪"),
    ("mute", "🔇"),
    ("necktie", "👔"),
    ("ok_hand", "👌"),
    ("outbox_tray", "📤"),
    ("package", "📦"),
    ("page_facing_up", "📄"),
    ("paperclip", "📎"),
    ("passport_control", "🛂"),
    ("pencil2", "✏️"),
    ("pizza", "🍕"),
    ("point_right", "👉"),
    ("poop", "💩"),
    ("pray", "🙏"),
    ("pushpin", "📌"),
    ("question", "❓"),
    ("rainbow", "🌈"),
    ("recycle", "♻️"),
    ("rewind", "⏪"),
    ("robot", "🤖"),
    ("rocket", "🚀"),
    ("rotating_light", "🚨"),
    ("safety_vest", "🦺"),
    ("satellite", "📡"),
    ("scissors", "✂️"),
    ("see_no_evil", "🙈"),
    ("seedling", "🌱"),
    ("shield", "🛡️"),
    ("snowflake", "❄️"),
    ("sparkles", "✨"),
    ("speech_balloon", "💬"),
    ("star", "⭐"),
    ("stethoscope", "🩺"),
    ("sunny", "☀️"),
    ("tada", "🎉"),
    ("technologist", "🧑‍💻"),
    ("telescope", "🔭"),
    ("test_tube", "🧪"),
    ("thinking", "🤔"),
    ("thread", "🧵"),
    ("thumbsdown", "👎"),
    ("thumbsup", "👍"),
    ("triangular_flag_on_post", "🚩"),
    ("trophy", "🏆"),
    ("truck", "🚚"),
    ("twisted_rightwards_arrows", "🔀"),
    ("wastebasket", "🗑️"),
    ("wave", "👋"),
    ("wheelchair", "♿"),
    ("white_check_mark", "✅"),
    ("wrench", "🔧"),
    ("x", "❌"),
    ("zap", "⚡"),
];

/// `code`（コロン無し）の絵文字。
pub fn emoji_for(code: &str) -> Option<&'static str> {
    SHORTCODES
        .binary_search_by(|(name, _)| (*name).cmp(code))
        .ok()
        .map(|index| SHORTCODES[index].1)
}

/// `:rocket: を直す` → `🚀 を直す`。表に無い物・コロンの間に英小文字 / 数字 / `_` `+` `-` 以外が
/// ある物はそのまま。
pub fn expand_shortcodes(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let code_end = after
            .find(|character: char| {
                !(character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '_' | '+' | '-'))
            })
            .unwrap_or(after.len());
        let closed = after[code_end..].starts_with(':');
        match emoji_for(&after[..code_end]).filter(|_| closed && code_end > 0) {
            Some(emoji) => {
                out.push_str(emoji);
                rest = &after[code_end + 1..];
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_for_lookup() {
        assert!(
            SHORTCODES.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "二分探索するので名前の昇順"
        );
    }

    #[test]
    fn shortcodes_become_emoji_and_the_rest_stays() {
        assert_eq!(expand_shortcodes(":rocket: を直す"), "🚀 を直す");
        assert_eq!(
            expand_shortcodes("fix :bug: and :sparkles:"),
            "fix 🐛 and ✨"
        );
        assert_eq!(expand_shortcodes(":+1: ok"), "👍 ok");
        assert_eq!(
            expand_shortcodes("12:30:45 に集合"),
            "12:30:45 に集合",
            "時刻"
        );
        assert_eq!(expand_shortcodes(":unknown_code: x"), ":unknown_code: x");
        assert_eq!(expand_shortcodes("a: b :c"), "a: b :c");
        assert_eq!(expand_shortcodes("::rocket::"), ":🚀:");
        assert_eq!(expand_shortcodes("no colons"), "no colons");
    }
}
