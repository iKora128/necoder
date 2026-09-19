//! **実 `claude-agent-acp` に対する Chat プリセットの通し確認**（ROADMAP M16 C0）。
//!
//! 窓を出さずに、Chat モードが本番で使うのと同じ部品（`chat_core::preset` のプリセット、
//! `chat_core::policy` の裁定、`acp_client` の `session/new` / `session/load`）を実エージェントに
//! 繋いで、次を確かめる:
//!
//! 1. 相談にはテキストだけで答え、ファイルを作らない
//! 2. 成果物を頼むと `artifacts/` に 1 ファイルだけ書く
//! 3. 相談の途中（成果物がある状態での質問）でファイルを触らない
//! 4. 修正は同じファイルの編集で、コピーを作らない
//! 5. 道具は持たせた物だけ（Bash も、ユーザー設定の MCP も見えない）
//! 6. 渡した PDF を読める
//! 7. 渡していない場所への書き込みは拒否される
//! 8. **プロセスを立て直して `session/load` で再開しても** 2〜5 が保たれる
//!
//! 使い方: `cargo run -p chat_core --example probe_chat_preset`
//! `-- --compare` は同じ問いを Editor のスレッドと同じ作り方でも投げ、初回応答と文脈の量を比べる。
//! `-- --image` を付けると、画像つき prompt（ROADMAP M16 C4）の 1 ターンだけを確かめる。
//! 課金されるので CI では回さない（実エージェント・10 ターン前後）。一時ディレクトリだけを触る。

use acp_client::{AgentEvent, SessionCommand, SessionPreferences};
use chat_core::policy::{judge, Scope, Verdict};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TURN_TIMEOUT: Duration = Duration::from_secs(240);

struct Probe {
    chat_dir: PathBuf,
    attachments: Vec<PathBuf>,
    command_tx: futures::channel::mpsc::UnboundedSender<SessionCommand>,
    event_rx: futures::channel::mpsc::UnboundedReceiver<AgentEvent>,
    session_id: Option<String>,
    resumed: bool,
    tokens_used: u64,
}

struct Turn {
    text: String,
    tools: Vec<String>,
    denied: Vec<String>,
    first_chunk: Option<Duration>,
    total: Duration,
}

impl Probe {
    fn start(chat_dir: &Path, attachments: Vec<PathBuf>, resume: Option<String>) -> Probe {
        Self::start_with(chat_dir, attachments, resume, true)
    }

    /// `chat_preset = false` は Editor のスレッドと同じ作り方（プリセットなし）＝比較の基準。
    fn start_with(
        chat_dir: &Path,
        attachments: Vec<PathBuf>,
        resume: Option<String>,
        chat_preset: bool,
    ) -> Probe {
        let host = host::LocalHost::shared();
        let kind = acp_client::AgentKind::by_label("Claude Code").expect("Claude Code");
        let command = kind
            .resolve_command_on(host.as_ref(), chat_dir.to_path_buf(), None, None)
            .expect("解決に失敗")
            .expect("claude-agent-acp が導入されていない");
        let preferences = SessionPreferences {
            mode: Some(chat_core::preset::PERMISSION_MODE.to_string()),
            resume,
            // ユーザー設定の MCP は渡さない（Chat は空が既定）。
            mcp_servers: Vec::new(),
            preset: if chat_preset {
                chat_core::preset::session_preset(chat_dir, chat_core::date::Date::today(), "")
            } else {
                acp_client::preset::SessionPreset::default()
            },
            ..SessionPreferences::default()
        };
        let (command_tx, command_rx) = futures::channel::mpsc::unbounded();
        let (event_tx, event_rx) = futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let outcome = futures::executor::block_on(acp_client::run_session_on(
                Arc::clone(&host),
                command,
                preferences,
                command_rx,
                event_tx,
            ));
            if let Err(error) = outcome {
                eprintln!("session ended: {error:#}");
            }
        });
        Probe {
            chat_dir: chat_dir.to_path_buf(),
            attachments,
            command_tx,
            event_rx,
            session_id: None,
            resumed: false,
            tokens_used: 0,
        }
    }

    /// 1 ターン送って、終わるまでのイベントを畳む。権限リクエストは本番と同じ裁定で答える。
    fn turn(&mut self, prompt: &str) -> Turn {
        self.turn_with_images(prompt, Vec::new())
    }

    fn turn_with_images(&mut self, prompt: &str, images: Vec<acp_client::PromptImage>) -> Turn {
        use futures::StreamExt as _;
        let attachments_prefix: String = self
            .attachments
            .iter()
            .map(|path| format!("@{}\n", path.display()))
            .collect();
        let full_prompt = if attachments_prefix.is_empty() {
            prompt.to_string()
        } else {
            format!("{attachments_prefix}\n{prompt}")
        };
        println!("\n> {prompt}");
        let started = Instant::now();
        self.command_tx
            .unbounded_send(if images.is_empty() {
                SessionCommand::Prompt(full_prompt)
            } else {
                SessionCommand::PromptWithImages {
                    text: full_prompt,
                    images,
                }
            })
            .expect("送信路");
        let mut turn = Turn {
            text: String::new(),
            tools: Vec::new(),
            denied: Vec::new(),
            first_chunk: None,
            total: Duration::ZERO,
        };
        futures::executor::block_on(async {
            loop {
                if started.elapsed() > TURN_TIMEOUT {
                    panic!("ターンが時間切れ: {prompt}");
                }
                let Some(event) = self.event_rx.next().await else {
                    panic!("セッションが途中で終わった: {prompt}");
                };
                match event {
                    AgentEvent::SessionStarted {
                        session_id,
                        resumed,
                    } => {
                        println!("  session {session_id} resumed={resumed}");
                        self.session_id = Some(session_id);
                        self.resumed = resumed;
                    }
                    AgentEvent::AgentChunk(chunk) => {
                        turn.first_chunk.get_or_insert_with(|| started.elapsed());
                        turn.text.push_str(&chunk);
                    }
                    AgentEvent::ToolStarted(info) => {
                        let title = info.title.unwrap_or_default();
                        println!("  tool: {title} {:?}", info.locations);
                        turn.tools.push(title);
                    }
                    AgentEvent::Usage { used, .. } => self.tokens_used = used,
                    AgentEvent::PermissionRequest {
                        title,
                        kind,
                        paths,
                        options,
                        respond,
                        ..
                    } => {
                        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                        let verdict = judge(
                            kind,
                            &paths,
                            Scope {
                                chat_dir: &self.chat_dir,
                                attachments: &self.attachments,
                                write_grants: &[],
                            },
                        );
                        println!(
                            "  permission: {title} kind={kind:?} paths={paths:?} → {verdict:?}"
                        );
                        let wanted = match verdict {
                            // 確認が要る物は、この確認では「ユーザーが今回だけ許可した」ことにする。
                            Verdict::Allow | Verdict::Ask => acp_client::PermissionKind::Allow,
                            Verdict::Deny(_) => {
                                turn.denied.push(title.clone());
                                acp_client::PermissionKind::Reject
                            }
                        };
                        let index = options
                            .iter()
                            .position(|option| option.kind == wanted)
                            .unwrap_or(0);
                        respond.unbounded_send(index).ok();
                    }
                    AgentEvent::Notice(notice) => println!("  notice: {notice}"),
                    AgentEvent::Failed(error) => panic!("ターンが失敗: {error}"),
                    AgentEvent::TurnEnded { .. } => break,
                    _ => {}
                }
            }
        });
        turn.total = started.elapsed();
        let shown: String = turn.text.chars().take(240).collect();
        println!(
            "  < {}{}",
            shown.replace('\n', " "),
            if turn.text.chars().count() > 240 {
                "…"
            } else {
                ""
            }
        );
        println!(
            "  first chunk {:?} · total {:?} · context {} tokens",
            turn.first_chunk.unwrap_or_default(),
            turn.total,
            self.tokens_used
        );
        turn
    }
}

/// フォルダの中身（相対パス → 内容）。変わったか・増えたかを比べるための写し。
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, files);
            } else if let (Ok(relative), Ok(bytes)) =
                (path.strip_prefix(base), std::fs::read(&path))
            {
                files.insert(relative.to_string_lossy().to_string(), bytes);
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// 1 ページに 1 行だけ書いた最小の PDF（外部ツール無しで作る。xref のオフセットは計算する）。
fn minimal_pdf(line: &str) -> Vec<u8> {
    let stream = format!("BT /F1 24 Tf 72 720 Td ({line}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

struct Report {
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, name: &str, passed: bool, detail: impl std::fmt::Display) {
        println!("  [{}] {name} — {detail}", if passed { "ok" } else { "NG" });
        if !passed {
            self.failures.push(format!("{name}: {detail}"));
        }
    }
}

fn artifact_names(files: &BTreeMap<String, Vec<u8>>) -> Vec<&String> {
    files.keys().collect()
}

fn main() {
    let root = std::env::temp_dir().join(format!("necoder-chat-probe-{}", std::process::id()));
    let chat_dir = root.join("documents/necoder/2026-09-18 probe");
    let outside = root.join("outside");
    std::fs::create_dir_all(&chat_dir).expect("chat_dir");
    std::fs::create_dir_all(&outside).expect("outside");
    let pdf = outside.join("memo.pdf");
    std::fs::write(&pdf, minimal_pdf("The passphrase is KOTATSU-4721.")).expect("pdf");
    let forbidden = outside.join("not-attached.txt");
    println!("chat_dir = {}", chat_dir.display());

    let mut report = Report {
        failures: Vec::new(),
    };

    if std::env::args().any(|argument| argument == "--compare") {
        // 同じ問いを、Chat のプリセットと Editor のスレッドと同じ作り方の両方で投げる（ROADMAP M16 C6）。
        for (label, chat_preset) in [("chat preset", true), ("editor thread (no preset)", false)] {
            let mut probe = Probe::start_with(&chat_dir, Vec::new(), None, chat_preset);
            let turn = probe.turn("Rust の所有権って何？2 行で教えて");
            println!(
                "COMPARE {label}: first chunk {:?} · total {:?} · context {} tokens",
                turn.first_chunk.unwrap_or_default(),
                turn.total,
                probe.tokens_used
            );
            drop(probe);
            std::thread::sleep(Duration::from_secs(2));
        }
        return;
    }

    if std::env::args().any(|argument| argument == "--image") {
        // 単色の PNG を作って送り、色を答えさせる（貼り付けたスクリーンショットと同じ経路）。
        use base64::Engine as _;
        let red = image::RgbImage::from_pixel(96, 96, image::Rgb([225, 30, 30]));
        let mut png = std::io::Cursor::new(Vec::new());
        red.write_to(&mut png, image::ImageFormat::Png)
            .expect("png");
        let mut probe = Probe::start(&chat_dir, Vec::new(), None);
        let turn = probe.turn_with_images(
            "この画像は何色？色の名前を一語で答えて",
            vec![acp_client::PromptImage {
                mime_type: "image/png".to_string(),
                data: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
            }],
        );
        let lowered = turn.text.to_lowercase();
        report.check(
            "貼り付けた画像をエージェントが見ている",
            lowered.contains('赤') || lowered.contains("red"),
            turn.text.replace('\n', " "),
        );
        drop(probe);
        std::process::exit(if report.failures.is_empty() { 0 } else { 1 });
    }

    // ---- 新規セッション ----
    let mut probe = Probe::start(&chat_dir, Vec::new(), None);

    let turn = probe.turn("Rust の所有権って何？2 行で教えて");
    let files = snapshot(&chat_dir);
    report.check(
        "相談にはテキストだけで答える",
        files.is_empty() && !turn.text.trim().is_empty(),
        format!("files={:?}", artifact_names(&files)),
    );

    probe.turn("ポモドーロタイマー作って。作業 25 分・休憩 5 分で");
    let made = snapshot(&chat_dir);
    let only_one_html_in_artifacts = made.len() == 1
        && made
            .keys()
            .all(|name| name.starts_with("artifacts/") && name.ends_with(".html"));
    report.check(
        "成果物は artifacts/ に 1 ファイルだけ",
        only_one_html_in_artifacts,
        format!("files={:?}", artifact_names(&made)),
    );

    let turn = probe.turn("ところで、ポモドーロってなんで 25 分なの？");
    report.check(
        "相談の途中でファイルを触らない",
        snapshot(&chat_dir) == made && !turn.text.trim().is_empty(),
        format!("tools={:?}", turn.tools),
    );

    probe.turn("アクセントカラーを青にして");
    let recolored = snapshot(&chat_dir);
    report.check(
        "修正は同じファイルの編集（コピーを作らない）",
        recolored.len() == made.len() && recolored.keys().eq(made.keys()) && recolored != made,
        format!("files={:?}", artifact_names(&recolored)),
    );

    let turn = probe.turn("もうちょっと見やすくできる？");
    let after_ambiguous = snapshot(&chat_dir);
    report.check(
        "曖昧な依頼でファイルを増やさない",
        after_ambiguous.keys().eq(recolored.keys()),
        format!(
            "files={:?} edited={}",
            artifact_names(&after_ambiguous),
            after_ambiguous != recolored
        ),
    );
    let _ = turn;

    let turn =
        probe.turn("いま自分が使えるツールの名前を、カンマ区切りで全部書いて。説明は要らない。");
    let lowered = turn.text.to_lowercase();
    report.check(
        "道具は持たせた物だけ（Bash も MCP も見えない）",
        !lowered.contains("bash") && !lowered.contains("mcp__") && lowered.contains("read"),
        turn.text.replace('\n', " "),
    );

    let turn = probe.turn(&format!(
        "{} に hello と書いたファイルを作って",
        forbidden.display()
    ));
    report.check(
        "渡していない場所への書き込みは拒否される",
        !forbidden.exists() && !turn.denied.is_empty(),
        format!("denied={:?} exists={}", turn.denied, forbidden.exists()),
    );

    let session_id = probe.session_id.clone().expect("session id");
    let tokens_first = probe.tokens_used;
    drop(probe); // 送信路を閉じる＝エージェントのプロセスが終わる

    // ---- プロセスを立て直して session/load で再開（PDF を渡す）----
    std::thread::sleep(Duration::from_secs(2));
    let before_resume = snapshot(&chat_dir);
    let mut probe = Probe::start(&chat_dir, vec![pdf.clone()], Some(session_id));

    let turn = probe.turn("渡した PDF に書いてある合言葉を、そのまま答えて");
    report.check("session/load で再開できた", probe.resumed, "resumed");
    report.check(
        "渡した PDF を読める",
        turn.text.contains("KOTATSU-4721"),
        turn.text.replace('\n', " "),
    );

    probe.turn("さっきのタイマーの一番下に、小さく「necoder probe 4721」という文字を足して");
    let after_resume = snapshot(&chat_dir);
    report.check(
        "再開後も同じファイルを編集する（前の会話を覚えている）",
        after_resume.keys().eq(before_resume.keys()) && after_resume != before_resume,
        format!("files={:?}", artifact_names(&after_resume)),
    );

    let turn =
        probe.turn("いま自分が使えるツールの名前を、カンマ区切りで全部書いて。説明は要らない。");
    let lowered = turn.text.to_lowercase();
    report.check(
        "再開後も道具は持たせた物だけ（_meta が session/load にも効いている）",
        !lowered.contains("bash") && !lowered.contains("mcp__") && lowered.contains("read"),
        turn.text.replace('\n', " "),
    );

    println!(
        "\ncontext tokens: 新規の終わり {tokens_first} / 再開の終わり {}",
        probe.tokens_used
    );
    drop(probe);
    if report.failures.is_empty() {
        println!("\n全部通った。残骸: {}", root.display());
    } else {
        println!("\n{} 件 NG:", report.failures.len());
        for failure in &report.failures {
            println!("  - {failure}");
        }
        println!("残骸: {}", root.display());
        std::process::exit(1);
    }
}
