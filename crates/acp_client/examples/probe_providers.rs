//! 実アダプタ（claude-agent-acp）に `providers/set` を送り、`providers/list` で反映を確かめる確認用プローブ
//! （issue #38 H3）。**セッションは開かず prompt も送らない** — 宛先とキーは偽物（`example.invalid` と
//! ダミーの文字列）で、API へは何も行かない。
//!
//! 使い方: `cargo run -p acp_client --example probe_providers [アダプタの実行ファイル]`
//! （省略時の起動はスレッドと同じ解決: 公開レジストリのキャッシュの版 → 組み込みカタログ）
use acp_client::connections::ProviderRoute;
use std::collections::BTreeMap;

fn main() {
    let host = host::LocalHost::shared();
    let kind = acp_client::AgentKind::by_label("Claude Code").expect("組み込みのエージェント");
    let cwd = std::env::temp_dir();
    let registry = acp_client::registry::load_cached();
    let mut command = kind
        .resolve_command_on(host.as_ref(), cwd, None, registry.as_ref())
        .expect("解決に失敗")
        .expect("Claude Code の ACP アダプタが導入されていない");
    if let Some(path) = std::env::args().nth(1) {
        command.path = std::path::PathBuf::from(path);
        command.args.clear();
    }
    println!("起動: {} {:?}", command.path.display(), command.args);

    let route = ProviderRoute {
        base_url: "https://example.invalid/anthropic".to_string(),
        headers: BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer necoder-probe-dummy".to_string(),
        )]),
    };
    let probe = futures::executor::block_on(acp_client::probe_providers(&command, &route))
        .expect("providers/* を確かめられない");
    if !probe.advertised {
        println!("このアダプタは agentCapabilities.providers を広告しない（env で渡す経路になる）");
        return;
    }
    println!("providers/list（set の前）: {}", probe.before);
    println!("providers/list（set の後）: {}", probe.after);
    let current = &probe.after["providers"][0]["current"];
    let reflected = current["apiType"] == "anthropic" && current["baseUrl"] == route.base_url;
    println!(
        "{}",
        if reflected {
            "反映された: providers/set の宛先が providers/list の current に出た（ヘッダは返らない）"
        } else {
            "反映されていない"
        }
    );
    if !reflected {
        std::process::exit(1);
    }
}
