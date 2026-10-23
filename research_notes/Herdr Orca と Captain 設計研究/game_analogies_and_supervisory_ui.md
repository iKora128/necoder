# ゲーム UI のアナロジーと「多数の半自律エージェントを監督する UI」の科学 — necoder Fleet / Captain への示唆

調査日: 2026-09-20（URL はすべてこの日に取得。取得できなかったものは各 Gaps に明記）

**結論（先に）**: ゲームの**機能語彙**（番号キー・次の要対応へ 1 キー・1 体 1 タイル・常時見える資源・発令キュー）は借りる価値がある。人間工学の知見（fan-out・状況認識・アラーム管理）と同じ方向を向いており、necoder は既に半分以上を持っている。ゲームの**衣装**（ピクセルオフィス・Warcraft 風スキン・地形マップ・スコア）は着せない。Gas Town の受容が示すとおり、フィクションの名詞は 1 つ増えるごとに「頭の中で翻訳する税」になり、性能予算と「色は識別」の掟にも反する。

**根拠の強さの凡例**: 【確立】査読論文・規格 / 【実務知】業界の実践・開発者の一次発言 / 【意見】エッセイ・ブログ / 【自己申告】作者・ベンダーの主張。

**前提にしたローカル文書**（読んだだけ・変更なし）: `/Users/daichi/Work/experience/necoder/docs/FLEET-V2.md`（§3.2 要対応の並び・§3.6 ブリッジ・§4.2 Blocked は 15 秒超で脈動を速く・§5.2 権限表・§7 キー・§10 F3 で `fleet_mascot` を削除済み・§10.5 設定 `reduce_motion`）、`/Users/daichi/Work/experience/necoder/docs/UI-SPEC.md`（§1.3 色の許可リスト = トークンメーターはスレッド色可・状態に色相禁止 / §6 状態ドット = Working は点字スピナー約 8 コマ/秒・Blocked は半円 + 速い脈動・レールでは Working を省く）、`/Users/daichi/Work/experience/necoder/docs/research/fleet-ux-2026-09.md`（cmux の ⌘⇧U = 最新未読へ）。

---

## KQ1. RTS / ストラテジーの HUD 語彙 — なぜ多数ユニットの指揮に効くのか

### Takeaway
RTS の HUD が効く理由は絵柄ではなく、(1) 注意が要る場所へ 1 キーで飛べる、(2) ユーザーが割り当てた番号が動かない、(3) 遊休と異常だけが常時カウントされる、(4) 資源が常に見える、の 4 点に尽きる。necoder の ⌘⇧U・⌘1..9・要対応・トークン常時表示はこの語彙と同型で、足りないのは「番号の固定割当」「手空きの件数」「発令キュー」。

### Cited Findings
- 【実務知】StarCraft II の公式ガイド。遊休ワーカー（採集も建設も移動もしていない）の**件数アイコンがミニマップの上に常時出る**。F1 で遊休ワーカーを選択して巡回、Ctrl+F1 で全選択。コントロールグループは Ctrl+数字で割当・数字で選択、"Pressing the number key twice in quick succession centers your view on the group you've assigned to it."。ユニットの通報（"We're under attack" 等）は **Space で発生地点へ視点移動**し、連打で "a history of unit transmissions for up to 8 transmissions" を巡回。Backspace で拠点を巡回 — [Blizzard, Game Guide: Special Control（最終更新の記載 2012-01-02）](https://news.blizzard.com/en-us/article/4552955/game-guide-special-control)
- 【実務知】Josh Bycer の RTS UI 論（2015-03-24）。"the less time you need to flip between bases and the battlefield is key for an effective UI."、"idle workers, unit attacks, and building completion should have some visual mention on the UI so that the player has an idea of what's happening."、"learning hotkeys are vital for taking that step from novice to master."。Planetary Annihilation を例に "a RTS with a minimalist UI is not a good thing" とミニマル過ぎる UI を批判。コマンドと状態が画面の複数箇所に散ることも「注意の分割」として戒める — [Game Developer, UI Strategy Game Design Dos and Don'ts](https://www.gamedeveloper.com/design/ui-strategy-game-design-dos-and-don-ts)
- 【実務知】Supreme Commander（2007）の strategic zoom。個々のユニットの接写から全マップまで継ぎ目なくズームでき、引き切るとミニマップの全画面版になり、ユニットはアイコンで示される。80km 四方のマップ・プレイヤーあたり千単位のユニットを扱うための仕組み — [Wikipedia, Supreme Commander](https://en.wikipedia.org/wiki/Supreme_Commander_(video_game))。縮小時の strategic icon は種別を形で示し、アイコン下の小さな白いタブで tech level を示す — [Supreme Commander Wiki, Strategic icon（日付なし・本文は 402 で未取得、検索要約による）](https://supcom.fandom.com/wiki/Strategic_icon)
- 【実務知】Homeworld（1999）は "Sensors Manager" で全マップと可視の全艦を一望するモードに切り替えられる — [Wikipedia, Homeworld](https://en.wikipedia.org/wiki/Homeworld)
- 【実務知】Celia Hodent（Epic Games で Fortnite の UX を統括・心理学 PhD）。知覚は主観・注意は希少・記憶は不確か、が出発点。Fortnite の罠アイコンは弾薬や木に誤読され、テストで熊罠の絵に直した。使いやすさの柱は signs & feedback / clarity / form follows function / consistency / minimum workload / error prevention & recovery / flexibility — [Hodent, Understanding the Success of Fortnite（2019-07-22・2020-06-20 更新）](https://celiahodent.com/understanding-the-success-of-fortnite-ux/)。書籍は [The Gamer's Brain（CRC Press, 2017）](https://www.routledge.com/The-Gamers-Brain-How-Neuroscience-and-UX-Can-Impact-Video-Game-Design/Hodent/p/book/9780367638184)
- 【実務知】同じく Fortnite。赤いサインを「被弾とその方向」以外ほぼ全部外し、被弾フィードバックを照準の近くに置いた。敵の体力バーは橙に変えた。結果として「気づかないまま死ぬ」率が大きく下がった — [Hodent, Understanding the success of Fortnite, part 2（Medium 転載・公開日未確認）](https://medium.com/ironsource-levelup/understanding-the-success-of-fortnite-a-ux-and-psychology-perspective-part-2-eea824ae63ca)
- 【実務知】Into the Breach の Justin Ma: "As a game design principle, we would sacrifice cool ideas for the sake of clarity every time."。明快に見せられない武器や攻撃パターンは切った。アニメ付きツールチップが「最も重要な決定」 — [Game Developer（Alex Wawro, 2018-03-05）](https://www.gamedeveloper.com/design/-i-into-the-breach-i-dev-on-ui-design-sacrifice-cool-ideas-for-the-sake-of-clarity-every-time-)。GDC 2019 の設計ポストモーテムは [GDC Vault](https://www.gdcvault.com/play/1025772/-Into-the-Breach-Design)
- 【実務知】Steve Swink の game feel の定義は "real-time control of virtual objects in a simulated space, with interactions emphasised by polish"。実時間の応答 = 知覚→判断→操作→新しいフィードバックの補正サイクルが 100ms 未満。polish（アニメ・音・パーティクル・画面揺れ）は 3 要素のうちの 1 つで、制御とは別物 — [Wikipedia, Game feel（書籍 Game Feel は 2008/2009 刊）](https://en.wikipedia.org/wiki/Game_feel)
- 【意見】David Hoang（2025-05-25）。RTS で育った世代の技能はエージェント采配に転用できる。"Your compute...are the modern equivalents of wood, gold, and stone"。ただし本人が "I'm not suggesting our work interfaces will look precisely like a RTS game" と断っている — [Proof of Concept, Real-Time Strategy games and AI interfaces](https://www.proofofconcept.pub/p/real-time-strategy-games-and-ai-interfaces)
- 【自己申告】AgentCraft の Ido Salomon（AI Coding Summit 2026・2026-02-26）: "RTS games are glorified task management simulators"。タスクを生む・進捗を見る・実時間で管理する、の 3 つがエージェント采配と一致するという主張 — [GitNation](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration)

### Inferences
- necoder の既存機能と RTS 語彙の対応はすでに密: ⌘⇧U / ⏎ = Space（アラートへ）、⌘1..9 = コントロールグループ、要対応 = アラート一覧、ニュース = 通報履歴、⌘0 + ブリッジ = Sensors Manager、舞台 1/2/3 列 = ズーム段、トークン常時表示 = 資源カウンタ。**足すのは装置であって世界観ではない**。
- コントロールグループの要点は「ユーザーが割り当て、勝手に動かない」こと。FLEET-V2 には ⌘1..9 の記述が 2 通りあり（§7 は「Task 行 N へ」、§3.2 の F1 訂正は「レールの並び」）、要対応の並び替えで行順が変わるリストに連番を振ると押した先が毎回変わる。番号を Task に固定で割り当て（作成順かピン）、2 度押しで composer にフォーカス、が SC2 の意味論に沿う。
- 遊休の扱いは写像に注意が要る。RTS の遊休ワーカーは即損失だが、コーディングエージェントの Idle は「次の指示待ち」であって損失とは限らない。重いのは Blocked（KQ4 の待ち時間）。「手空き N」は件数を静かに出すだけにして、巡回キーは要対応と分けるのが筋。
- Fortnite の「赤を 1 つの意味に取っておく」は、necoder の「色は識別に取っておく」と同じ構造の判断。ゲーム UX の第一人者の実例が掟を支持している。
- Swink の整理に従えば、necoder が借りるべきは real-time control（入力レイテンシ・1 キー移動）であり、polish（演出）は別勘定。性能予算を削ってまで足すものではない。
- Bycer のミニマル批判は「情報を隠すな」であって「飾れ」ではない。Into the Breach の原則と合わせると、ゲーム側の第一線は「密度は高く・装飾は低く」に立っている。

### Gaps
- Age of Empires IV / Company of Heroes の UI を一次資料（開発者講演・公式 UI 解説）で裏取りできなかった。検索では「ピリオドキーで遊休ワーカー選択」等の断片しか出ず、出典が特定できないので本文から外した。
- StarCraft II のアラート通知の間引き（同種通報のクールダウン）を公式資料で確認できなかった。
- interfaceingame.com / gameuidatabase.com はスクリーンショット集で、設計根拠の記述は得られなかった。
- 「RTS の HUD 装置が多数ユニット管理の成績を上げる」ことを直接測った査読研究は見つけていない。ここの根拠は実務知と開発者の発言が中心。

---

## KQ2. 艦隊 / クルー指揮ゲームはもっと近い類推か — どの装置が「worktree のエージェント + Captain」に写るか

### Takeaway
いちばんきれいに写るのは EVE Online の艦隊 UI（少数の型付き要請 + 受信フィルタ + 履歴 + watch list）と RimWorld の「顔の列 + 右のアラート一覧」、Football Manager の責任分担画面。どれも necoder の要対応カード・ピン・Captain 権限表と同型で、借りるのは構造だけでよい。FTL の船内図のような「絵」は写像はきれいでも描く必然がない。

### Cited Findings
- 【実務知】EVE Online の艦隊 UI。艦隊ウィンドウは Fleet / Broadcasts / Respawn / Formations / Advert / Find Fleets の 6 タブ。broadcast は 8 種の固定語彙（Enemy Spotted / Need Armour / Need Shield / Need Capacitor / In Position / Need Backup / Hold Position / Location）で、種別ごとに届く範囲が決まっている。受信側は種別ごとに受ける/受けないを選べ（ロジ艦でなければ Need Armor を切る等）、履歴タブで種別ごとに背景色を変えられる。送信スコープは My group / Everyone / Superiors。watch list は "allows a maximum of 100 pilots at any one time"。階層は Fleet Commander → Wing Commander → Squad Commander → メンバーで、編成ビューは指揮職だけが見られ、部下しか見えない — [EVE University Wiki, Fleet interface（wiki・日付なし）](https://wiki.eveuniversity.org/Fleet_interface)
- 【実務知】FTL: Faster Than Light はポーズ可能なリアルタイム戦闘で、プレイヤーは一時停止して状況を評価し命令を入れる。戦闘中は動力の配分とクルーの部屋への配置で船のシステムを回す — [Wikipedia, FTL: Faster Than Light](https://en.wikipedia.org/wiki/FTL:_Faster_Than_Light)
- 【実務知】RimWorld のコロニストバー（画面上端の顔の列）は、背景の暗さで気分、表示で負傷/睡眠/徴兵状態、服装で装備を伝える。設計原則は「一瞥で読める要素は "clearly but also in a manner that does not require the player to look for very much longer than a simple glance"」「"often done visually rather than via text"」。サブメニューに埋もれているが咄嗟に要る情報は一瞥要素に上げよ、と勧める — [Moonlit Development, Rimworld's At-A-Glance User Interface Elements（Dylan Moon, 2017-12）](https://www.moonlitdevelopment.com/development-blog/2017/12/25/the-designers-folder-rimworlds-at-a-glance-user-interface-elements)
- 【実務知】RimWorld の mod「Radius UI - Colonist Bar」の説明文。襲撃中にバー全体が赤い壁になる代わりに、**いま自分を必要としている 1 人の名前だけを 1 本の帯で順に出す** — [Steam Workshop（公開日未確認）](https://steamcommunity.com/sharedfiles/filedetails/?id=3788278207)
- 【実務知・検索要約のみ】Factorio のアラート。旧仕様は点滅するアラート枠を押すと「最重要」の地点をマップで開くだけだった。新仕様は Alerts Overview が開き、惑星別・種別別の一覧になり、アラートをピン留めできる — [Factorio Wiki, Alerts（本文は SSL エラーで未取得）](https://stable.wiki.factorio.com/Alerts)、[Friday Facts #400（該当号は未確認）](https://www.factorio.com/blog/post/fff-400)
- 【実務知】Football Manager の Staff Responsibilities。責任の項目ごとに「自分で持つ / スタッフに委任」を 1 クリックで切り替える（メディア対応をアシスタントマネージャーへ、スカウト会議をチーフスカウトへ、等）。委任の出来はスタッフの能力値で決まる — [GameRant, FM26 How to Delegate Staff Responsibilities（2025–26）](https://gamerant.com/football-manager-26-fm26-how-delegate-staff-responsibilities/)、[Passion4FM, Staff Responsibilities Explained（日付未確認）](https://www.passion4fm.com/football-manager-staff-responsibilities-backroom-advices-explained/)
- 【自己申告】AgentCraft は Warcraft III 風で、主エージェント = Hero、サブエージェント = Peon、建物 = 能力、repo のファイル = 地形、エージェントの探索 = 戦場の霧、Space = 次の優先項目、ライフサイクルに効果音。デモでは「5 秒で 3 体のブロックを解いた」 — [GitNation（2026-02-26）](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration)、[getagentcraft.com](https://www.getagentcraft.com/)

### Inferences
- EVE の broadcast は「エージェント側から上がる、型の決まった助けの要請」で、necoder の Blocked（承認待ち）そのもの。学ぶ点は 3 つ: **語彙を少数に固定する**（necoder の要対応 5 種と同じ）、**一覧の項目から直接操作できる**（インライン承認と同じ）、**種別ごとに受信を切れる**（necoder に未実装。イベント種別ごとの通知設定に当たる）。
- watch list = ピン（◫）、broadcast history = ニュース、Fleet Commander = Captain、と 1 対 1 で写る。EVE の編成ビューが「指揮職だけ・部下だけ」なのは、Captain に渡す事実層を絞る設計（FLEET-V2 §5.6）と同じ発想。
- RimWorld の 2 点セット（常時見える顔の列 + 押すと現場へ飛ぶアラート一覧）は、Fleet サイドバーの「Task 行 + 要対応」と同型。Radius UI の工夫は KQ4 の alarm flood 対策と同じで、「全員が赤」を「いま要る 1 件」に畳む。
- Football Manager の画面が示すのは「委任の境界が一覧で見える」こと。necoder の Captain 権限表（§5.2）は文書にしかなく、UI には「采配のみ · 統合と承認は人間が操作」の 1 行だけ。読み取り専用の一覧として見せる余地がある。Integrate と承認は不変条件なのでトグルにはしない。
- FTL の「部屋 = システム / クルー = 配置」は「worktree = 部屋 / エージェント = クルー」と写るが、船の断面図を描いても Task 一覧より情報が増えない。借りるなら「有限の動力を配るバー」= 予算配分の見せ方と、「止めて考える」= 艦隊の一時停止（根拠は KQ4 の稼働率）。
- AgentCraft の「repo を地形にする」は、コードに地理が無い以上は意味のない空間を作る。霧の機能的な芯は「最後に見た状態は古い」の表示で、necoder では done-unread と digest の時刻が同じ役をする。

### Gaps
- Pikmin（キャプテンが小さな働き手を率いる）と Star Trek 系ブリッジクルーゲーム（Artemis / Star Trek: Bridge Crew）は、UI 設計を論じた信頼できる資料を見つけられなかった。名前（Captain / bridge）の連想以上の根拠は無い。
- EVE の watch list 上限 100 は wiki の現行記述。CCP の開発ブログ（[Now, where did I put that fleet?](https://www.eveonline.com/news/view/now-where-did-i-put-that-fleet)）は未読。
- Factorio のアラート仕様は検索要約しか得られていない。該当する Friday Facts の号数は未確認。
- FTL の UI を開発者（Subset Games）が語った一次資料は得られず、Wikipedia の記述に留まる。

---

## KQ3. MMO のレイド / パーティフレーム — 5 / 10 / 25 / 40 人で読める理由

### Takeaway
レイドフレームが読めるのは「1 人 1 タイル・位置固定・連続量は棒 1 本・離散状態は決まった枠・注意不要は減光・人数でレイアウトを切り替える」からで、necoder のサイドバー Task 行（54px・3 段）は既にほぼこれ。ただし設計原則の出典はフォーラムとブログ止まりで、学術的な裏付けは「プレイヤーは標準 UI の情報量を足りないと感じ、密度を自分で選びたがる」という調査 1 本。軍用 UAV 研究が同じ形（1 機 1 アイコン・状態は数段階）で大きな効果を報告しているのが補強になる。

### Cited Findings
- 【確立】WoW の UI mod 調査（Game Studies 12(2), 2012-12・2007 年 N=283 / 2011 年 N=58）。**標準 UI は "too little information" だと答えた人が 78.4%（2007）/ 68.4%（2011）**。利用の多い mod カテゴリは interface bars・map・**raid**・inventory で、いずれも「既にあるが見えにくい大量のデータを表に出す」もの。プリセット一式より個別の mod を選びたい人が 91.3% / 92.6%。回答者は男性 88–91%・平均 24 歳・週 24.5 時間プレイ・72–79% が自分を PC の上級者と評価 — [Targett, Verlysdonk, Hamilton, Hepting, A Study of User Interface Modifications in World of Warcraft](https://gamestudies.org/1202/articles/ui_mod_in_wow)
- 【実務知】ヒーラーが一瞥で要る情報は、全員の HP バー・マナ・上書き可能な HoT・重要なデバフ（解除種別で色分け）。レイドフレームは画面中央寄り、キャラの斜め隣に置く — [MMO-Champion, Elements of a proper healer UI?（フォーラム・日付未確認）](https://www.mmo-champion.com/threads/2524877-Elements-of-a-proper-healer-UI)
- 【実務知】VuhDo は 5 人 / 10 人 / 25 人用のレイアウトを切り替えられ、各フレームにバフ・クールダウン・HoT などのインジケータを載せる — [Healing Spec, Vuhdo UI Walkthrough（2012-04-10）](https://healingspec.com/2012/04/10/188/)
- 【実務知】Grid は **status（何が起きているか）と indicator（どこに出すか）を分け**、ユーザーが status を任意の indicator に割り当てる。各 status に色・優先度などの設定がある — [Grid wiki FAQ（GitHub・日付なし）](https://github.com/phanx-wow/Grid/wiki/FAQ)
- 【実務知】Grid / VuhDo はミニマルな箱型の路線。Blizzard は後に Grid 型のレイドフレームを標準に取り込んだ — [MMO-Champion, Grid vs VuhDo vs Default Raid Frames（フォーラム・日付未確認）](https://www.mmo-champion.com/threads/1477684-Healing-Grid-vs-VuhDo-vs-Default-Raid-Frames)
- 【確立】軍用 UAV 監督の SME 研究（Frontiers in Psychology, 2016）。健全性の指標を 4 カテゴリにまとめ Intact / Warning / Fault の 3 段階で示すと、1 機の点検が 5 分 → 2 分。**指揮地図上に UAV 1 機につき 1 つの「正常」アイコンを置くと、経路逸脱の検知が 3 分 → 5 秒**。健全性の時系列グラフで故障源の特定が 13 分 → 5 分未満 — [Porat, Oron-Gilad, Rottem-Hovev, Silbiger, Supervising and Controlling Unmanned Systems](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290)

### Inferences
- レイドフレームの写像: HP バー → context window 使用量、キャストバー → 実行中のツール呼び出し + 経過、デバフ枠 → blocked / failed / radar ✗、距離外の減光 → 注意不要（Idle・確認済み Done）、aggro → 要対応の先頭。向きだけ逆で、HP は減ると危険・context は増えると危険。満杯側に目盛りを打てば誤読しない。
- 5 / 10 / 25 / 40 で読める理由を実務知からまとめると: (a) 位置が固定（空間記憶で探さずに済む）、(b) 連続量は棒 1 本、(c) 離散状態は少数の決まった枠に優先度つきで出す、(d) 人数でレイアウトを切り替える、(e) 注意不要は薄くする。(a) は出典を取れていないので推論。
- necoder の Task 数は現実には 3〜10（KQ4 の上限）。だから 25 人レイド用の極小タイルは要らず、**N ≤ 5 は現行の 3 段行、N ≥ 10 で 1 段のコンパクトタイルに落とす**程度でよい。
- Grid の status → indicator 分離は、necoder の「登録式を優先」（CLAUDE.md のコーディング規約）と同じ形。タイルの枠（スロット）を固定し、状態の供給側を登録式にすれば、Git health や radar を後から足してもタイルの読み方が変わらない。
- WoW 調査の「個別に選びたい 9 割」は密度のカスタマイズ需要を示すが、回答者は平均 24 歳・上級者の男性に偏る。レイドフレームの読み方は、プロの開発者一般の共通語彙ではない。

### Gaps
- レイドフレームの可読性を直接測った査読研究は見つからなかった。根拠はフォーラム・ブログ・アドオンの wiki。
- 位置固定が探索時間を縮めるという主張の出典（空間的一貫性の HCI 文献）は今回取得していない。
- Blizzard の公式な設計意図（Dragonflight 以降の Edit Mode など）は未調査。

---

## KQ4. 多数の自律エージェント / ロボット / UAV の監督に関する人間工学の知見

### Takeaway
1 人が回せる数は「放置できる時間 ÷ 1 回の介入にかかる時間」で決まり、UI の仕事は介入時間を縮めること、Captain の仕事は放置できる時間を延ばすこと。直接操作は 3 体まで、承認つき（management-by-consent）で 4〜5 体、例外時だけ（management-by-exception）で 8〜12 体、監視だけなら 15 体前後、が UAV 研究の相場で、コーディングの実務家の自己申告（3〜4 本）と合う。アラートは「行動できるものだけ・10 分に 10 件を超えたら洪水」。状態を形と動きで示す掟は知覚研究と両立するが、**識別色で塗った状態グリフは色相のばらつきが形の読み取りを妨げうる**、という非自明な注意点がある。

### Cited Findings
- 【確立】Sheridan（1992）の監督制御。人間の監督者の 5 機能は planning / teaching（プログラミング）/ monitoring / intervening / learning。監督制御は「それ以外は自動で動く系に対して、人間が起動・監視・調整を行う活動」 — [Wikipedia, Supervisory control](https://en.wikipedia.org/wiki/Supervisory_control)、[Sheridan, Automation（HFES）](https://www.hfes.org/Portals/0/Documents/Sheridan.pdf)
- 【確立】Olsen & Wood（CHI 2004）。1 人が操作できるロボット数を fan-out と呼び、ロボットの activity time と interaction time を結ぶ式を定義。neglect tolerance が高く interaction time が短いほど fan-out は上がる — [ACM DL, Fan-out: measuring human control of multiple robots](https://dl.acm.org/doi/10.1145/985692.985722)。式は Fanout = AT/IT（AT = 次の入力まで進捗し続ける時間）。待ち時間を入れた拡張は Fan-out = NT/(IT + WT) — [Int. J. Humanoid Robotics, On the Redefinition of Fan Out Metric（2010・検索要約による）](https://www.worldscientific.com/doi/10.1142/S0219843610002143)
- 【確立】Goodrich & Schultz の HRI サーベイ（2007）。neglect tolerance = ロボットを放置できる時間。fan-out は「独立で同種のロボットを 1 人が管理できる数の上限」だが、"it is clear that fan-out is only a designer guideline"。問いを「1 人で何台」から「一定数のロボットを回すのに何人要るか」へ移す立場も紹介 — [Human-Robot Interaction: A Survey（PDF 本文で確認）](https://faculty.cs.byu.edu/~mike/mikeg/papers/HRISurvey.pdf)
- 【確立・検索要約による】Cummings らの一連の研究。ミッション管理ループだけに人間が関わり他を高度に自動化すれば、実験では 1 人で Tactical Tomahawk を最大 12 発まで扱えた — [Cummings & Guerlain, Human Factors 49(1), 2007](https://doi.org/10.1518/001872007779598109)。**稼働率（busy time）70% が性能劣化の閾値**として妥当で、配員予測に一般化できそう。敵防空制圧の想定では容量は UCAV 5 機 — [Predicting Operator Capacity for Supervisory Control of Multiple UAVs（Springer, 2007）](https://link.springer.com/chapter/10.1007/978-3-540-72696-8_2)。待ち行列・複数機の相互作用・状況認識の喪失を入れると**予測容量は最大 67% 下がり、主因は状況認識の喪失による遅れ** — [DTIC AD1015470（PDF は 403 で未取得）](https://apps.dtic.mil/sti/tr/pdf/AD1015470.pdf)
- 【確立】Porat ら（2016・現役オペレーターによる多段階研究）。**監視（健全性モニタ）なら中程度の自動化と適切な UI で 15 機まで**。17 機を超えると高負荷と「制御を失う感覚」。自動化が最小だと 5 機でも 1 時間で「難しく消耗する」。**制御（任務とペイロード）は 3 機が限界**で、支援ツール無しで 3 本の映像を見る条件では熟練 10 人中 7 人がシナリオの途中で降りた。文献引用として management-by-exception で 8–12 機、management-by-consent で 4–5 機 — [Frontiers in Psychology（PMC4878290）](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290)
- 【確立】Ruff, Narayanan, Draper（2002）。手動 / management-by-consent（自動化が提案し、人間の同意で実行）/ management-by-exception（一定時間内に拒否しなければ自動実行）を比較。**consent が、より自律的な水準にもより手動の水準にも、明確な性能上の優位を示した**。監督する UAV が増え、自動化の信頼性が下がるほど性能は落ちる — [Presence 11(4), 2002](https://direct.mit.edu/pvar/article-abstract/11/4/335/18417/Human-Interaction-with-Levels-of-Automation-and)
- 【確立】Lewis（2013）のレビュー。command complexity で整理: 各機が同じ形の注意を要るなら O(n)、機体間に依存があれば O(>n)、**自律的に協調する群と人間がやりとりするなら O(1)**（群の大きさに依らない） — [Human Interaction With Multiple Remote Robots](https://journals.sagepub.com/doi/10.1177/1557234X13506688)
- 【確立・要旨のみ】Goodrich, Shields, Adams（AI Magazine, 2026-08-24）。human–robot ratio は監督制御の設計目安として有用だが、相互依存するメンバーの柔軟な組織では誤解を招きうる。人間・ロボット・AI エージェントのチームを捉える新しい枠組み（attention-demand economy）が要る — [Reframing the human–robot ratio（本文は 403）](https://onlinelibrary.wiley.com/doi/10.1002/aaai.70086)
- 【確立】Endsley（1995）。状況認識 = 環境の要素の知覚（L1）・意味の理解（L2）・近い将来の予測（L3）。L1 が欠けると上が成り立たない — [Toward a Theory of Situation Awareness in Dynamic Systems](https://www.researchgate.net/publication/210198492_Endsley_MR_Toward_a_Theory_of_Situation_Awareness_in_Dynamic_Systems_Human_Factors_Journal_371_32-64)。自動化は操作者を out-of-the-loop にし、故障時の引き継ぎを遅らせる。制御への関与の度合いが状況認識の喪失を和らげる — [Endsley & Kiris, Human Factors, 1995](https://journals.sagepub.com/doi/10.1518/001872095779064555)
- 【確立】Bainbridge（1983）"Ironies of Automation"。自動化が進むほど人間には「めったに介入の要らない系を見張り続ける」消耗する監視と、稀だが決定的な介入が残る。技能は使わなければ落ちる — [Wikipedia, Ironies of Automation](https://en.wikipedia.org/wiki/Ironies_of_Automation)
- 【確立】知覚。前注意的に検出できる特徴は色相・輝度・向き・大きさ・曲率・閉合・**flicker・動きの方向と速度**など（200–250ms 以内）。**非対称な干渉**: "Background variations in colour interfere with a viewer's ability to identify the presence of individual shapes...If colour is held constant across the display, these same shape patterns are immediately visible. The interference is asymmetric: random variations in shape have no effect on a viewer's ability to see colour patterns."。flicker の弁別は中心視なら周波数差 2–5% で足りるが、**周辺視では 100% 以上の差が要る**。結合ターゲット（赤い丸を赤い四角と青い丸の中から探す等）は前注意的に見つからず逐次探索になる。変化盲: 瞬き・サッカード・画面の途切れの間の変化は、注意を向けていた場所以外は見落とす — [Healey, Perception in Visualization（NCSU・継続更新の Web 版・日付なし）](https://www.csc2.ncsu.edu/faculty/healey/PP/)
- 【実務知】NN/g。変化盲は「注意の焦点から遠い領域の変化を無視する傾向」。アニメーションは変化の合図になるが、競合するアニメが多いと注意が薄まる。変える箇所は一度に 1 つ・同じ領域にまとめる — [Change Blindness in UX: Definition（公開日未確認）](https://www.nngroup.com/articles/change-blindness-definition/)、[The Role of Animation and Motion in UX（公開日未確認）](https://www.nngroup.com/articles/animation-purpose-ux/)
- 【確立・規格（二次資料経由）】アラーム管理。EEMUA 191（1999）/ ISA-18.2（2009）/ IEC 62682（2014）。長期平均で**操作者 1 人あたり 1 時間に約 6 件（10 分に 1 件）が受容可能、12 件/時が管理できる上限**。**10 分間に 10 件超は alarm flood**。アラームは本当に対応が要る時だけ出す — [Chemical Engineering, Alarm Management By the Numbers（日付未確認）](https://www.chemengonline.com/alarm-management-numbers/)、[ISA-18.2 vs EEMUA 191（ブログ・日付未確認）](https://www.merobix.com/blog/isa-18-2-vs-eemua-191)
- 【実務知】Google SRE 本（O'Reilly, 2016）。"Every page should be actionable"。"When pages occur too frequently, employees second-guess, skim, or even ignore incoming alerts, sometimes even ignoring a 'real' page that's masked by the noise."。人を呼ぶのは高くつく。緊急でない問題はメールではなくダッシュボードへ。まず「何が壊れたか（症状）」、次に「なぜ（原因）」 — [Monitoring Distributed Systems](https://sre.google/sre-book/monitoring-distributed-systems/)
- 【実務知・二次資料】Airbus の dark cockpit（"lights out"）。A310（1980 年代初め）から。系統が正常なら何も点かない。点灯は手動選択・オーバーライド・非正常だけ。ねらいは視覚ノイズを減らし、対処や認識が要る時だけ注意を引くこと — [Airflow, The Dark Cockpit Philosophy（2025-01-16）](https://www.airflow.blog/2025/01/16/the-dark-cockpit-philosophy-enhancing-efficiency-and-safety-in-modern-aviation/)、[AviaPro, Airbus "Lights Out" vs. Boeing "Quiet and Dark"（日付未確認）](https://aviaproconsulting.com/article/post/engineering-spotlight-the-ergonomics-of-annunciation-%E2%80%93-airbus-lights-out-vs-boeing-quiet-and-dark)
- 【確立】Shneiderman（1996）の mantra は "overview first, zoom and filter, then details on demand"。7 つのタスクは overview / zoom / filter / details-on-demand / relate / history / extract — [The Eyes Have It（IEEE Visual Languages 1996）](https://www.cs.umd.edu/~ben/papers/Shneiderman1996eyes.pdf)
- 【確立】Mackay（TOCHI 6(4), 1999）。パリの航空管制官チームを 4 か月観察。"we do not know enough to simply get rid of paper strips, nor can we easily replace the physical interaction between controllers and paper strips."。提案は紙のストリップを残したまま計算機への入口にすること — [Is Paper Safer? The Role of Paper Flight Strips in Air Traffic Control](https://www.lri.fr/~mackay/pdffiles/TOCHI2000.SaferPaper.pdf)
- 【確立】周辺 / アンビエント表示。Matthews らは peripheral display の要件を abstraction / notification / transitions に整理。Pousman & Stasko（AVI 2006）は ambient 情報系を「重要だが致命的でない情報・周辺と焦点を行き来できる・更新は微かな変化で（邪魔しない）」と定義 — [A Taxonomy of Ambient Information Systems](https://faculty.cc.gatech.edu/~stasko/papers/avi06.pdf)
- 【実務知】Stephen Few（2006）の「ダッシュボードの 13 の過ち」。1 画面を超える・文脈不足・過剰な詳細・意味のない表現の多様さ・重要データを強調できていない・無用な装飾・色の誤用など。自動車の計器のような「別の物に見せるための装飾」は目的の邪魔になる — [Common Pitfalls in Dashboard Design（Perceptual Edge）](https://www.perceptualedge.com/articles/Whitepapers/Common_Pitfalls.pdf)
- 【確立・小規模】AgentGUI（ETH Zürich, arXiv 2026-07-28）。長時間走るエージェントの観察と操舵の GUI。activity feed・**タイムライン概観**・コンソールを持ち、ダッシュボードはエージェントを「ピクセルアートのオフィスの作業員（1 体 1 デスク）」で見せる。N=8（修士・博士課程）で**理解課題が 38% 速く（90 秒 vs 145 秒, p=0.023）、正答率 80% → 93%（p=0.031）**、NASA-TLX の mental demand・frustration・effort が有意に低下。著者は小規模で同質な標本を限界に挙げ、複数エージェント規模の実時間操舵の検証を今後の課題とする — [AgentGUI: An Interface for Observing and Steering Long-Running AI Agents](https://arxiv.org/abs/2607.26300)
- 【確立】Microsoft Research ほか（arXiv 2026-02-18・ユーザー調査 3 本・対象は Computer Use Agent）。現行の検証手段は "cumbersome, limiting their efficacy"。提案した行動トレース UI は誤り発見の時間を縮めたが、**参加者の自信は上がったのに最終的な正確さは意味のある改善をしなかった** — [Grunde-McLaughlin et al., Overseeing Agents Without Constant Oversight](https://arxiv.org/abs/2602.16844)
- 【実務知】並列コーディングエージェントの実務家。Simon Willison（2025-10-05）: "AI-generated code needs to be reviewed, which means the natural bottleneck on all of this is how fast I can review the results."。並列化しやすいのは調査・PoC・既存コードの理解・小さな保守・仕様が明確な作業 — [Embracing the parallel coding agent lifestyle](https://simonwillison.net/2025/Oct/5/parallel-coding-agents/)。Addy Osmani（2026-04-07）: "My ceiling for a typical session is somewhere around three to four threads depending on complexity."、見張りの負担は "ambient anxiety tax"、"By noon you're not reviewing anything carefully anymore - you're just accepting output."（正式な研究の引用は無い） — [Your parallel Agent limit](https://addyosmani.com/blog/cognitive-parallel-agents/)。Willison は 4 体並列で「午前 11 時には消耗しきる」とも語った — [Lenny Rachitsky の X 投稿（2026 年春・日付未確認）](https://x.com/lennysan/status/2039845666680176703)

### Inferences
- **fan-out の式を necoder に当てる**: 上げる手段は 2 つだけ。IT（1 回の介入にかかる時間）を縮める = 要対応カードのインライン承認・⌘⇧U・Captain の推薦。NT（放置できる時間）を延ばす = Captain による采配・承認ポリシー。ゲーム HUD の語彙で IT 短縮に効くものは借りる価値があり、効かないものは装飾。
- **舞台の上限 3 枚は Porat の「制御は 3 機まで」と一致**する。サイドバー（監視）は 15 前後まで伸ばせるが、承認を人間が持つ限り（consent）、実効の並走数は 4〜5 本が相場。実務家の 3〜4 本とも合う。ただし UAV からコーディングへの転用は未検証。
- **Captain の理論的な位置づけ**は Lewis の O(n) → O(1) への移行。人間が群と話せば群の大きさは負担に効かない。ただし Integrate と承認が人間に残るので、その分は O(n) のまま。fan-out をさらに上げたいなら、効く梃子は UI の演出ではなく承認ポリシー（management-by-exception）。代償は状況認識の低下と慢心（Endsley・Bainbridge）。Ruff の結果は、現行の「Captain は推薦だけ・応答は人間」（§5.5）を支持する。
- **状況認識の 3 段を面に割る**: L1 = 状態グリフ・要対応の件数、L2 = digest（いま何を / どう終わったか）・phase ピル、L3 = Captain が何を出して何を待っているか + 見込み。necoder に欠けているのは L3。
- **状態グリフと識別色の干渉**（掟に直結）: necoder の状態グリフはスレッド / Task の識別色で描かれるので、行ごとに色相がばらつく。Healey の非対称干渉に従えば、このばらつきは「どれが Blocked か」という形のパターンの読み取りを妨げうる。打ち手は掟の内側にある: (1) 位置で符号化する（要対応を最上段に集める = 既にやっている。位置は最強のチャネル）、(2) 概観面では状態グリフを中立色（fg0 / fg1）で識別色ドットの隣に分けて描き、状態の形の集合を色相一定にする、(3) 輝度（減光）を使う。輝度は色相と別の前注意特徴なので掟に触れない。
- **動きは 1 クラスだけにする**: 現行は Working（8 コマ/秒のスピナー）も Blocked（速い脈動）も動く。周辺視では flicker の差が 2 倍以上ないと区別できないので、5 体が回っている中の 1 体の脈動は飛び出して見えにくい。dark cockpit に従い、概観面では Working を静かに（低振幅か静止の「稼働中」形）、目立つ動きは Blocked 専用にするのが筋。レールでは既に Working を省いているので、同じ判断を編隊図とサイドバーの非フォーカス時に広げる話。実測で確かめる価値がある（Working 7 + Blocked 1 で、composer を見ながら気づくまでの秒数）。
- **アラートの上限**: 規格の目安（6〜12 件/時）に照らすと、5 体のエージェントが各々 10 分に 1 回承認を求めれば 30 件/時で上限を超える。だから「Captain の推薦」「常に許可」「同種の承認を 1 枚に束ねる」は便利機能ではなく、洪水を防ぐ必須部品。
- **AgentGUI の結果の読み方**: 効いたのは activity feed とタイムライン（構造化された概観）で、ピクセルオフィスの比喩そのものの効果は切り分けられていない。Grunde-McLaughlin の「自信だけ上がる」と合わせると、きれいな可視化は過信を生む危険がある。要約の横に必ず一次情報（diff・実行ログ）への 1 クリックを置く。
- Mackay の知見の転用は弱いが、要対応カードを人間が手で並べ替える / ピン留めできることには外部記憶としての価値がありうる。

### Gaps
- NASA ミッションコントロールのコンソール慣行（コールサイン・ループ・go/no-go）と NOC の運用知見は、設計根拠として引ける一次資料を取得できなかった。
- Olsen & Wood の原典 PDF、Crandall & Cummings（HRI 2007）の本文は 403 で未取得。「IT は約 16 秒で一定・最適 fan-out は 4〜6」という記述をブログで見たが、原典で確認できなかったので本文から外した。Crandall らの変形 FO = NT/IT + 1 も今回は未確認。
- Goodrich ら（2026）の attention-demand economy は要旨しか読めていない。
- Lewis らの「単一アラーム vs 複数アラーム（キュー）」実験（[Springer, 2014](https://link.springer.com/chapter/10.1007/978-3-319-07515-0_50)）は認証壁で読めず、要対応キューの並べ方の直接の根拠は得られなかった。
- 単独人間・複数ロボット系の最新レビュー（[Ergonomics, 2025](https://www.tandfonline.com/doi/full/10.1080/00140139.2025.2529316)）も 403。
- **複数の LLM コーディングエージェントを 1 人が監督する条件で、UI の違いを比べた対照実験は見つからなかった**（AgentGUI は単一軌跡の理解課題・N=8）。この領域の根拠は UAV / ロボットからの類推と実務家の自己申告。
- 「形と動き vs 色相」で状態を符号化した場合の検出成績を直接比べた研究は未調査。上の干渉の議論は Healey の総説からの推論。

---

## KQ5. ゲーム / 空間メタファーを既に使っている AI エージェントツールと、その受容

### Takeaway
受容ははっきり割れている。調整役を 1 つの窓口にする構造（Gas Town の Mayor、Antigravity の Inbox）は好評で、フィクションの名詞（Mad Max の役名）は「頭の中で翻訳し続ける」と不評。ピクセルオフィス系は GitHub では人気（Pixel Agents 9.3k stars）だが HN ではほぼ無反応で、「監視の本体」ではなく「楽しい上着」として受け取られている。RTS を全面に出した製品（VibeCraft・Ralv.ai・AgentCraft）は話題先行で、効果を測った独立の評価は見つからない。主要な商用製品（Cursor 3・Antigravity）は素のリスト / タイル / 受信箱を選んでいる。

### Cited Findings
- 【実務知】Gas Town（Steve Yegge, 2026-01）。Mad Max 風の役割階層（Mayor / Polecats / Refinery / Witness / Deacon / Dogs / Crew）を tmux で回す。Mayor はコードを書かず、ユーザーと話し、作業を切って配る — [Yegge, Welcome to Gas Town](https://steve-yegge.medium.com/welcome-to-gas-town-4f25ee16dd04)。【自己申告】Yegge 自身は「みんな Mayor と働くのが好きで、polecat も convoy も sling も筋が通っている」と書く — [Gas Town Emergency User Manual](https://steve-yegge.medium.com/gas-town-emergency-user-manual-cf0e4556d74b)
- 【意見】Maggie Appleton（2026-01）。"this thing fits the shape of Yegge's brain and no one else's. I'd categorise that as a moderate design fail."。HN コメントの引用として "The number of overlapping and ad hoc concepts in this design is overwhelming."。一方で階層は負荷を下げると評価: "With the Mayor as your single interface, that overhead disappears."。エージェントがコードを書く時代は設計がボトルネックになる、とも論じる — [Gas Town's Agent Patterns, Design Bottlenecks, and Vibecoding at Scale](https://maggieappleton.com/gastown)
- 【実務知】HN スレッド（取得時 354 points・224 comments）。命名については「参照が全部頭上を通り過ぎた」「名前を頭の中でいつも翻訳しなければならない」という不満と、"software can have personality again, yay" という歓迎が並ぶ。ある開発者は用語を標準的な分散システムの語（orchestrator・worker pool・merge queue）に訳し直して説明した。役割階層そのものは「チームの側面はだいたい合っている」と評価する声もある（引用は WebFetch の抜粋による） — [Hacker News, Welcome to Gas Town](https://news.ycombinator.com/item?id=46458936)
- 【実務知】Tenzin Wangdhen の 1 週間の試用記（2026-02-19）。良: Mayor が単一の調整点になり 10 以上のセッションを手で追わずに済む・polecat ごとの git worktree で本当に並列・夕食前に 7 件投げて 6 件の PR がマージ済み。悪: **可視性が薄い（いつマージされたか・なぜ止まったかが分からない）**・孤児プロセス 141 個・tmux の習熟が前提。醜: Mad Max の体系を丸ごと受け入れる必要・`--dangerously-skip-permissions` 前提・高コスト。結局やめて beads だけ残した — [Gas Town: The Good, The Bad, The Ugly](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)
- 【意見】Mad Max の比喩は Mayor・Rig・Refinery・Convoy までは続くが、"Dogs"（Mick Herron の Slow Horses 由来）で体系が崩れる — [Today in Tabs, All Gas Town, No Brakes Town（日付未確認）](https://www.todayintabs.com/p/all-gas-town-no-brakes-town)
- 【実務知】Pixel Agents（Pablo De Lucca・MIT）。取得時 **9.3k stars / 1.5k forks**。標語は "The most playful way to orchestrate your agents"。Claude Code の端末ごとにピクセルキャラが付き、書く / 読む / コマンド実行でアニメが変わり、入力待ち・承認待ちは**吹き出し**、任意で**通知音**。ロードマップに「実際のゲームメカニクス（HP バー・スコア）」。webview 内で軽量なゲームループ・canvas 描画・BFS 経路探索を回す — [GitHub, pixel-agents](https://github.com/pablodelucca/pixel-agents)。JetBrains 版も出ている — [JetBrains Marketplace, Pixel Office](https://plugins.jetbrains.com/plugin/31298-pixel-office--ai-agent-visualizer)。一般誌も取り上げた — [Fast Company, This charming pixel art game solves one of AI coding's most annoying UX problems（本文は 403・日付未確認）](https://www.fastcompany.com/91497413/this-charming-pixel-art-game-solves-one-of-ai-codings-most-annoying-ux-problems)
- 【実務知】同じ Pixel Agents の Show HN（2026-09 前半）は **3 points** でほぼ無反応 — [Hacker News](https://news.ycombinator.com/item?id=49648023)。【意見】「ピクセルオフィスは今後 3〜5 年は産業用ダッシュボードを置き換えない。組織の監視には可愛さ以上のものが要る」。一方でシミュレーションゲームで育った世代には直感的な視覚言語、とも — [Kan Yuenyong, Pixel Agents and the Future of Agentic AI Monitoring（Medium・日付未確認）](https://sikkha.medium.com/pixel-agents-and-the-future-of-agentic-ai-monitoring-8810175f1d53)。類似の個人プロジェクトに [claude-office](https://github.com/paulrobello/claude-office)
- 【確立・小規模】学術側でも ETH Zürich の AgentGUI（2026-07）が「ピクセルアートのオフィス」をダッシュボードに採用（評価の詳細は KQ4） — [arXiv 2607.26300](https://arxiv.org/abs/2607.26300)
- 【自己申告】RTS 全面型。VibeCraft（"Starcraft for AI agents"）の HN 投稿は 2 points・コメントは作者の 1 件だけ — [Hacker News（2025-10 頃）](https://news.ycombinator.com/item?id=45575842)。Ralv.ai は 3D 空間・ドラッグ選択・ズーム・ミニマップで 50 体超を扱うと謳う。紹介記事は「端末文化の GUI への抵抗」「200 ユニット超で比喩が保つか」を未解決とし、暗号資産トークンが紐づく点にも触れる — [self.md, Ralv.ai（2026-02-10・2026-09-19 更新）](https://self.md/tools/ralv-ai/)。AgentCraft は KQ2 のとおり — [GitNation（2026-02-26）](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration)
- 【実務知】Agent of Empires（aoe）は名前こそゲームのもじりだが、中身は tmux + git worktree のセッション管理（Rust 製 TUI + Web）。状態は running / waiting / idle / error。Mozilla.ai が支援 — [agent-of-empires.com](https://www.agent-of-empires.com/)、[GitHub](https://github.com/agent-of-empires/agent-of-empires)
- 【実務知】Google Antigravity（2025-11）の Agent Manager は "Mission Control" と "Inbox" の比喩。各タスクをメールのように扱う。レビューは「受信箱という発想が brilliant」「各タスクの状態が一目で分かる」 — [Google Antigravity Blog](https://antigravity.google/blog/introducing-google-antigravity)、[DEV, An Honest Review of Google Antigravity](https://dev.to/fabianfrankwerner/an-honest-review-of-google-antigravity-4g6f)
- 【実務知】Cursor 2.0（2025 年秋）はエージェントを並べるサイドバーを入れ、Cursor 3（2026-04-02）で全 repo のセッションを一覧する Agents Window、3.1 でタイル配置。ゲーム的な比喩は無い。評者は「1 人の開発者には Agents Window の得は薄い。自分だけのタイルが並ぶだけ」とし、人間のレビュー時間が制約になると指摘 — [Cursor Docs, Agents Window](https://cursor.com/docs/agent/agents-window)、[The AI Signal, Cursor 3.0 Moved the Agents Out of the Sidebar（2026-04-21）](https://vibe.cerridan.com/posts/cursor-3-agents-out-of-sidebar/)
- 【実務知・一方的】Claude Code の npm パッケージ v2.1.88 のソースマップから、ユーザーごとに決定的に生成されるピクセルペット（12 種・レア度 5 段）の仕組みが見つかった（2026-03-30）。記事は好意的な反応だけを紹介している — [HackerNoon, Claude Code Leak Reveals Hidden Pixel Pet System](https://hackernoon.com/claude-code-leak-reveals-hidden-pixel-pet-system)

### Inferences
- Gas Town が示すのは「構造は当たり・衣装は外れ」。Mayor（= 単一の窓口になる調整役）は批判者も認める一方、名詞の群れは翻訳税になった。necoder の Captain は Mayor の構造だけを取り、語彙は「Task・branch・worktree・Integrate」と実物の名前のまま。**今の方針が正しい側にいる**。
- Gas Town をやめた人の一番具体的な不満は可視性（なぜ止まったか分からない）。necoder が差を付けられるのはフィクションではなく、Captain の発令キューと台帳の見える化。
- ピクセルオフィスの人気は「需要がある」証拠だが、需要の中身は楽しさ・愛着で、監督の成績ではない（HN での無反応、産業用にはならないという評）。Pixel Agents の状態通知は結局「吹き出し + 音」で、機能は necoder の要対応カードと同じ。
- 主要な商用製品が選んだのは仕事の比喩（受信箱・ミッションコントロール）か無比喩のリスト。ゲームの比喩を選んだのは個人開発と新興。市場はまだ判定を出していないが、プロ向け製品の主流は「静かな一覧」。
- Agent of Empires は「名前だけ遊び、UI は素」の例。名前の遊びは無害で、UI の遊びとは別の判断。
- necoder のピクセルアートの猫は、ブランド・空状態・オンボーディングなら Pixel Agents 的な愛着を安く得られる。状態の伝達路にはしない。FLEET-V2 の F3 で `fleet_mascot` を管制面から外した判断と整合する。

### Gaps
- Pixel Agents・AgentCraft・Ralv.ai の利用者による実質的な議論（Reddit・HN の長いスレッド）は見つからなかった。「何が明快で何が gimmick だったか」の利用者の声は、Gas Town を除いて薄い。
- Fast Company の記事と "God Mode UX: Why Your Next Interface Will Look More Like StarCraft Than Slack"（[Medium](https://medium.com/sadasant/god-mode-ux-why-your-next-interface-will-look-more-like-starcraft-than-slack-12498eb274d4)）は 403 で本文未読。
- Devin・Codex アプリ・Conductor の複数エージェント表示の受容は今回取得できていない。
- Yegge の続報（[Gas Town: from Clown Show to v1.0](https://steve-yegge.medium.com/gas-town-from-clown-show-to-v1-0-c239d9a407ec)・[Welcome to Gas City](https://steve-yegge.medium.com/welcome-to-gas-city-57f564bb3607)）で命名を整理したかどうかは未確認。
- "AI Town" / generative agents 系の可視化をコーディングエージェント管理に使った例の評価は見つからなかった。
- Claude Code のペット機能が実際にどう提供され、どう受け取られたかは HackerNoon の 1 本だけで、裏が取れていない。

---

## KQ6. リスク — 比喩のずれ・ゲーミフィケーションへの反発・学習コスト・アクセシビリティ・性能

### Takeaway
比喩が害になるのは、装飾が先に立つ時・現実の制約まで輸入する時・ユーザーを子供扱いする時・ラベルと挙動がずれる時。良い比喩は初心者の足場になり、上達したらショートカットで迂回できる。悪い比喩は檻になる。設計者の比喩とユーザーの比喩はそもそも一致しないことが多い（CHI 2026）。開発者向けツールのゲーミフィケーションには、行動を歪めたという自然実験の証拠がある。動きで状態を示す設計は、WCAG と性能予算の両方から「静的な形だけでも読める」ことを要求される。

### Cited Findings
- 【意見・権威】Jakob Nielsen（2025-08-14）。比喩が効く仕組みはスキーマの活性化・言語と視覚の二重符号化・チャンキング・予測処理。失敗の型は、装飾優先（写実的な質感は画面を食う視覚ノイズ）・制約の輸入（仮想オフィスは直接行ける所へ歩かせる）・恩着せがましさ（Clippy）・期待とのずれ（"folder" と書いてタグのように振る舞う）。"The computer should be better than reality, not an exact mirror image."。良い比喩は上達後にキーボードショートカット等で迂回でき、"Failed metaphors often lock users into a single mode of interaction, becoming a prison"。失敗例は Microsoft Bob・Magic Cap（家 / 部屋の UI が余計な移動を足した）・3D ファイルシステム。成功例はデスクトップ・フォルダ・ゴミ箱・カート・タブ・クリップボード。擬人化アシスタントは "raise false social expectations"、提案が外れると "feel condescending" — [Metaphor in UX Design](https://www.uxtigers.com/post/metaphor)
- 【確立】Bullock, Landay, Bernstein（CHI 2026・arXiv 2026-03-29）。ChatGPT・Twitter・YouTube について、ユーザーの比喩 554 件と設計側の比喩 21 件を比較。"design metaphors often do not match the metaphors that users use to describe their experiences"。一致していても全員に響くわけではない — [Comparing Design Metaphors and User-Driven Metaphors for Interaction Design](https://arxiv.org/abs/2603.27908)
- 【確立】Moldon, Strohmaier, Wachs（ICSE 2021）。GitHub が 2016 年 5 月に連続コントリビューション日数のカウンタを予告なく外した自然実験。長い streak が減り、週末の活動と「1 日 1 コミットだけの日」が減った。"gamification can steer the behavior of software developers in unexpected and unwanted directions."。ゲームのための最適化であって仕事の質のためではなかった — [It Will Never Work in Theory による要約（2021-10-03）](https://neverworkintheory.org/2021/10/03/how-gamification-affects-software-developers.html)、[論文 PDF](https://johanneswachs.com/papers/msw_icse21.pdf)
- 【意見・予測】Gartner（2012-11）は「2014 年までにゲーミフィケーションを入れたアプリの 80% が、主に設計の悪さで目的を果たせない」と予測。ポイント・バッジ・順位表という表面の仕掛けへの固執が原因とした（後追いの検証は見当たらない） — [TechCrunch（2012-11-27）](https://techcrunch.com/2012/11/27/badges-beware-80-of-gamification-apps-will-end-up-being-losers-says-gartner)
- 【確立・規格】WCAG 2.1 SC 2.2.2 Pause, Stop, Hide（Level A）。自動で始まり、**5 秒を超えて続き、他のコンテンツと並行して表示される**動き・点滅・スクロールには、一時停止・停止・非表示の手段が要る（その動きが本質的な場合を除く）。"those with attention deficit disorders, find blinking content distracting, making it difficult for them to concentrate on other parts of the web page."。進捗を示すアニメは、無いと「固まった」と誤解させるなら本質的と見なせる — [W3C, Understanding SC 2.2.2](https://www.w3.org/WAI/WCAG21/Understanding/pause-stop-hide.html)
- 【確立・規格】SC 2.3.3 Animation from Interactions（AAA）。操作で起きる動きのアニメは、本質的でない限り無効化できること。前庭障害では動きがめまい・吐き気・頭痛を起こす。色の変化・ぼかし・不透明度のフェードは「動きのアニメ」に当たらない — [W3C, Understanding SC 2.3.3](https://www.w3.org/WAI/WCAG21/Understanding/animation-from-interactions.html)。40 歳以上の成人の約 35% に前庭機能の問題、という数字は二次資料 — [TestParty（ブログ）](https://testparty.ai/blog/wcag-animation-interactions-guide)
- 【実務知】VS Code は 2017 年 3 月、フォーカス中・アイドルでも CPU を 13% 使うと報告された。原因は点滅カーソルを 500ms ごとではなく 16.67ms ごと（60fps）に描き直していたこと。回避策は `"editor.cursorBlinking": "solid"` — [microsoft/vscode #22900](https://github.com/microsoft/vscode/issues/22900)
- 【実務知】Pixel Agents は webview 内でゲームループ・canvas 描画・経路探索を常時回す設計 — [GitHub, pixel-agents](https://github.com/pablodelucca/pixel-agents)
- 【実務知】ゲーム経験の分布。米国では成人の 60% が毎週ゲームをし、平均年齢は 36 歳・平均プレイ歴 18 年。ただしブーマー以上の好みはパズル（73%）と skill & chance（55%） — [ESA, 2025 Essential Facts](https://www.theesa.com/resources/essential-facts-about-the-us-video-game-industry/2025-data/)。WoW の UI mod コミュニティは男性 88–91%・平均 24 歳・上級者 72–79% — [Game Studies 12(2)](https://gamestudies.org/1202/articles/ui_mod_in_wow)
- 【実務知】Few: ダッシュボードを別の物（自動車の計器盤など）に見せる装飾は、目的の邪魔になる。色は多用・誤用されがち — [Common Pitfalls in Dashboard Design（2006）](https://www.perceptualedge.com/articles/Whitepapers/Common_Pitfalls.pdf)
- 【実務知】ゲーム側も明快さを優先する: Into the Breach の "sacrifice cool ideas for the sake of clarity every time"、Fortnite の赤の整理と誤読アイコンの作り直し — [Game Developer（2018-03-05）](https://www.gamedeveloper.com/design/-i-into-the-breach-i-dev-on-ui-design-sacrifice-cool-ideas-for-the-sake-of-clarity-every-time-)、[Hodent（2019-07-22）](https://celiahodent.com/understanding-the-success-of-fortnite-ux/)

### Inferences
- **境界線は Nielsen の 1 行で引ける**: 比喩が「迂回できる足場」なら借りてよく、「その比喩を通らないと操作できない」なら衣装。⌘⇧U は StarCraft を知らなくても「次の要対応へ」として読める = 足場。歩くキャラで稼働を示すのは、その絵を読まないと状態が分からない = 檻。
- ゲームを知っている人が多数派でも、RTS やレイドを知っている人は別の話。ESA の数字は「ゲーム全般」で、好まれるジャンルは世代で大きく違う。レイドフレームの読み書きは若い男性の上級者に偏る。だから設計会話では「ユニットフレーム」「コントロールグループ」と呼んでよいが、**UI 文字列とドキュメントには出さない**。
- ゲーミフィケーション（スコア・連続記録・実績・エージェントの「レベル」）は、開発者の行動を歪めたという自然実験の証拠がある数少ない項目。Pixel Agents のロードマップの「HP バー・スコア」は necoder が行かない方向。トークンの資源バーは予算の可視化であって得点ではない、という線を守る。
- **動きの規律**: (1) 5 秒を超える並行アニメには止める手段が要る。Working のスピナーは「進捗を示すので本質的」と主張できるが、Blocked の脈動が何分も続くのは ADHD の利用者には妨害になる。脈動は数回で止め、形（半円）と位置（要対応の先頭）だけを残す案が規格にも dark cockpit にも沿う。(2) `reduce_motion` で全アニメが止まっても、形だけで 5 状態が区別できること。(3) フェード（不透明度）は「動き」に当たらないので、減光は reduce_motion 下でも使える。
- **性能**: VS Code の件は「小さな常時アニメが、フレームごとの再描画を通じてアイドルを殺す」典型。necoder は Fleet で誰かが Working の間ずっとスピナーを回すので、一日の大半でアイドルでなくなる可能性がある。全状態アニメを 1 本の低レートタイマー（≤ 8fps）に束ねる・見えている行だけ動かす・窓が非アクティブなら止める、が要点。ゲームループを持つ canvas 面（Pixel Agents 型）は idle メモリ / CPU 予算と両立しない。
- Hodent の誤読アイコンの話は状態グリフにも当てはまる。◐ / ⠿ / リング / 中空 が意図どおり読まれるかは、凡例を置くこととは別に、人に見せて確かめる価値がある。

### Gaps
- 「比喩は初心者を助けるが上級者を遅くする」を直接測った研究: 該当しそうな論文（[The long-term effects of integral versus composite metaphors on experts' and novices' search behaviors, Interacting with Computers, 2005](https://www.sciencedirect.com/science/article/abs/pii/S0953543805000056)）は 403 で読めず、知見を確認できていない。Blackwell の [The Reification of Metaphor as a Design Tool（TOCHI 2006）](https://dl.acm.org/doi/pdf/10.1145/1188816.1188820) も未読。比喩の効果に実証が乏しいという指摘は検索要約で見ただけ。
- 日本の開発者の RTS / MMO レイドのリテラシーに関するデータは見つからなかった（日本は RTS も PC MMO のレイドも欧米・韓国より浸透が浅いと思われるが、出典なし）。
- GPUI で常時アニメがアイドル CPU / メモリに与える影響の公開計測は見つからなかった。necoder 自身で測るしかない（`scripts/memory-usage.sh` に加えて、Fleet で Working N 本の CPU サンプル）。
- プロ向けツールでのスキューモーフィズムへの反発を定量的に示す資料は今回取得していない（Nielsen の論考に依拠）。

---

## KQ7. 命名 — 海軍 / 艦隊の比喩で統一すべきか、RTS やレイドと混ぜてよいか

### Takeaway
UI に出す比喩は今の海 / 空の系統（Fleet・Captain・ブリッジ・編隊・radar）で止め、RTS やレイドの語は設計会話の中だけに留める。それより大事なのは二層に分けること: 操作対象は実物の名前（Task・branch・worktree・Integrate・要対応）、比喩は既成の名前が無い役割と場所だけ。一貫した比喩でも語が増えれば古びる（Homebrew）。混ぜれば崩れる（Gas Town）。

### Cited Findings
- 【実務知】Kubernetes はギリシャ語 κυβερνήτης（操舵手・水先案内人）が語源で、ロゴは舵輪。Helm など周辺にも航海の主題が一貫して続く。同じ語根は cybernetics と governor の語源でもある — [Wikipedia, Kubernetes](https://en.wikipedia.org/wiki/Kubernetes)、[Nutanix, Why is Kubernetes Nautical（日付未確認）](https://www.nutanix.com/tech-center/blog/why-is-kubernetes-nautical)
- 【実務知】Homebrew のビール比喩（formula を tap から取って keg に注ぎ、keg を Cellar に並べる。bottle はビルド済みの keg、cask は GUI アプリ）。2023-03-07 の提案は "obtuse, and actively obfuscate package management concepts"、"the difference between kegs, taps, bottles and casks is extremely unclear, and can't easily be intuited from context" として Keg → Install・Tap → Repository・Bottle → Binary・Cask → App・Cellar → Install Dir への改名を求めた。closed as not planned — [Homebrew/brew #14912](https://github.com/Homebrew/brew/issues/14912)。それ以前に Homebrew 自身が、UI とドキュメントで使う比喩を Formula / Bottle / Cask / Tap に絞り、keg と cellar は利用者向けの出力に出さない、という整理を提案している — [Homebrew/brew #10798, Simplifying Homebrew terminology](https://github.com/Homebrew/brew/issues/10798)
- 【意見】Gas Town は Mad Max の語彙に別作品（Slow Horses）の語が混ざった所から体系が崩れる、という評 — [Today in Tabs](https://www.todayintabs.com/p/all-gas-town-no-brakes-town)。HN では用語を標準の分散システム用語へ訳して説明し直す人が出た — [Hacker News](https://news.ycombinator.com/item?id=46458936)
- 【意見・権威】成功した比喩（デスクトップ・フォルダ・ゴミ箱・タブ）は見た目ではなく機能の関係を写している。抽象的だから保っている。Microsoft Bob と Magic Cap は家と部屋を文字どおり再現して余計な移動を足した — [Nielsen, Metaphor in UX Design（2025-08-14）](https://www.uxtigers.com/post/metaphor)
- 【確立】設計側の比喩とユーザーが実際に使う比喩は一致しないことが多い — [Bullock et al., CHI 2026](https://arxiv.org/abs/2603.27908)
- 【実務知】Agent of Empires は名前だけがゲームのもじりで、UI の語は running / waiting / idle / error と素のまま — [agent-of-empires.com](https://www.agent-of-empires.com/)
- 【実務知】EVE の艦隊 UI の語は軍事的だが少数で固定（broadcast 8 種・階層 4 段） — [EVE University Wiki](https://wiki.eveuniversity.org/Fleet_interface)

### Inferences
- necoder の語を棚卸しすると既に混ざっている: 海 / 空（Fleet・Captain・ブリッジ・編隊図・radar）、劇場（舞台 / Stage）、家系（系譜 / Lineage）、新聞（ニュース）、簿記（台帳）、そして実物の名前（Task・branch・worktree・Integrate・要対応）。それでも破綻していないのは、**比喩の語が「場所と役割」に限られ、操作対象と動詞が実物の名前だから**。Gas Town は逆に、操作対象と動詞（bead・sling・convoy）まで比喩にした。
- 方針として言えること: (1) 操作対象と動詞に比喩を足さない。(2) 比喩は、既成の名前が無い役割（Captain）・モード（Fleet）・場所（ブリッジ）に限る。(3) 新しい比喩語を足す時は「その語を知らない人が、画面の文脈だけで意味を当てられるか」を試す（Homebrew の失敗は、文脈から当てられなかったこと）。(4) RTS・レイドの語（minimap・control group・unit frame・aggro）は GLOSSARY の code / 設計語に留め、UI 文字列にしない。
- 「海軍で統一」を徹底しすぎるのも危険。Homebrew は一貫していたのに、語が 6 つに増えた時点で説明責任を果たせなくなった。海の語は今の 5 つで足りており、増やす理由がない。「舞台」「系譜」を無理に海の語へ寄せる必要もない。どちらも文脈で当てられる。
- Kubernetes がうまく古びている理由の一部は、航海の比喩がブランドと周辺ツールの名前に留まり、日々操作する対象の名前（Pod・Node・Deployment・Service）が概ね素であることだと思われる（この点は今回の取得資料では未確認の背景知識）。
- 日本語 UI では Captain・Fleet・Task が英字のまま残る。日本語話者にとって「Captain」に海軍の含意は薄く、「まとめ役」程度に読まれる可能性が高い。比喩が弱く効くこと自体は害ではない（迂回できる足場）。

### Gaps
- 一貫した比喩（integral）と寄せ集めの比喩（composite）を比べた 2005 年の研究は本文を読めていない（KQ6 Gaps と同じ）。
- 製品の比喩が「うまく古びた / 古びなかった」を体系的に比べた研究は見つからず、事例（Kubernetes・Homebrew・Bob・Magic Cap・Gas Town）の並置に留まる。
- Homebrew #10798 の年月日と結末は本文を取得しておらず、検索要約による。

---

## KQ8. 総合 — ゲームアナロジーは良い案か（成果物 1〜3）

### Takeaway
良い案なのは「機能語彙を借りる」まで。necoder は既に RTS / 艦隊ゲームの HUD 語彙の大半を、ゲームの見た目なしで持っている。足す価値が根拠から言えるのは、ユニットフレーム・番号の固定割当・エスカレーション段階表・資源バー・Captain の発令キュー・事後タイムラインの 6 つと、状態の符号化の見直し。グラフ表示を「ゲームっぽく」する方向ではなく、**ノードを情報タイルにする**方向へ押すのが、作者の直感（グラフは艦隊を読めるようにするためにある）の正しい延長。

### Cited Findings
（この節の根拠は KQ1〜7 の再掲。要になるものだけ挙げる）
- 1 機 1 アイコンの概観で検知が 3 分 → 5 秒、状態を少数の段階にまとめて点検が 5 分 → 2 分 — [Porat et al., 2016](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290)
- fan-out は activity time と interaction time の比で決まり、あくまで設計者の目安 — [Olsen & Wood, CHI 2004](https://dl.acm.org/doi/10.1145/985692.985722)、[Goodrich & Schultz, 2007](https://faculty.cs.byu.edu/~mike/mikeg/papers/HRISurvey.pdf)
- management-by-consent が手動にも exception にも勝った — [Ruff et al., 2002](https://direct.mit.edu/pvar/article-abstract/11/4/335/18417/Human-Interaction-with-Levels-of-Automation-and)
- 色相のばらつきは形の読み取りを妨げる（逆は起きない）。周辺視の flicker 弁別には 100% 以上の差が要る — [Healey, Perception in Visualization](https://www.csc2.ncsu.edu/faculty/healey/PP/)
- 10 分に 10 件超はアラーム洪水。呼び出しはすべて行動可能であること — [Chemical Engineering](https://www.chemengonline.com/alarm-management-numbers/)、[Google SRE](https://sre.google/sre-book/monitoring-distributed-systems/)
- 遊休の件数表示・番号グループ・アラートへ 1 キー（履歴 8 件） — [Blizzard, StarCraft II Game Guide](https://news.blizzard.com/en-us/article/4552955/game-guide-special-control)
- Gas Town: 単一の窓口は好評、フィクションの名詞は翻訳税 — [Appleton](https://maggieappleton.com/gastown)、[Hacker News](https://news.ycombinator.com/item?id=46458936)、[Tenzin](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)
- 良い比喩は迂回できる足場、悪い比喩は檻 — [Nielsen, 2025-08-14](https://www.uxtigers.com/post/metaphor)。設計者とユーザーの比喩は一致しない — [Bullock et al., CHI 2026](https://arxiv.org/abs/2603.27908)
- 開発者ツールのゲーミフィケーションは行動を歪めた — [Moldon et al., ICSE 2021](https://johanneswachs.com/papers/msw_icse21.pdf)
- タイムラインと activity feed で理解が 38% 速く・正答 80 → 93%（N=8） — [AgentGUI, 2026-07](https://arxiv.org/abs/2607.26300)。ただし良い可視化は自信だけを上げうる — [Grunde-McLaughlin et al., 2026-02](https://arxiv.org/abs/2602.16844)
- 小さく疎なグラフ（約 20 ノード以下）は node-link 図が読みやすく、それを超えると行列表示がほとんどの課題で勝る（経路探索を除く） — [Ghoniem, Fekete, Castagliola, Information Visualization 4(2), 2005](https://journals.sagepub.com/doi/10.1057/palgrave.ivs.9500092)
- After-action review の 4 問: 何が起きるはずだったか / 実際に何が起きたか / なぜ差が出たか / 何を続け何を改めるか — [Wikipedia, After-action review](https://en.wikipedia.org/wiki/After-action_review)
- 小さな常時アニメがアイドル CPU を 13% 食った例 — [microsoft/vscode #22900](https://github.com/microsoft/vscode/issues/22900)。5 秒超の並行アニメには停止手段が要る — [WCAG SC 2.2.2](https://www.w3.org/WAI/WCAG21/Understanding/pause-stop-hide.html)

### Inferences

#### 成果物 1: 対応表（装置 → necoder Fleet の対応物 → 根拠 → 判定）

判定: **採用** = そのまま借りる / **翻案** = 芯だけ借りて形を変える / **回避** = 持ち込まない。「既」= necoder に既にある。

| # | ゲーム / HCI の装置 | necoder Fleet の対応物 | 根拠（賛 / 否） | 判定 |
|---|---|---|---|---|
| 1 | コントロールグループ（Ctrl+数字で割当・数字で選択・2 度押しで視点移動） | ⌘1..9（FLEET-V2 内に「Task 行 N」と「レールの並び」の 2 通りの記述） | 賛: ユーザーが割り当て、動かないから筋肉記憶になる [Blizzard](https://news.blizzard.com/en-us/article/4552955/game-guide-special-control)。ホットキーは初心者から熟練への段 [Bycer](https://www.gamedeveloper.com/design/ui-strategy-game-design-dos-and-don-ts)。否: 並び替わるリストの連番は押した先が変わる | 翻案: 番号は Task に固定。2 度押し = composer へ |
| 2 | 遊休ワーカー（件数を常時表示・F1 で巡回・Ctrl+F1 で全選択） | Idle / 手空きの件数を Captain バーに | 賛: 件数の常時表示 + 1 キー巡回 [Blizzard](https://news.blizzard.com/en-us/article/4552955/game-guide-special-control)。ねらい 1「待ち時間を埋める」に合う。否: コーディングの Idle は損失とは限らない。重いのは Blocked | 翻案: 件数だけ静かに |
| 3 | アラートへジャンプ（Space・履歴 8 件）/ AgentCraft の Space / cmux の ⌘⇧U | ⌘⇧U・サイドバーの ⏎（既） | 賛: 独立した 3 系統が同じ装置に収束 [Blizzard](https://news.blizzard.com/en-us/article/4552955/game-guide-special-control) [AgentCraft](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration)。interaction time の短縮 = fan-out の向上 [Olsen & Wood](https://dl.acm.org/doi/10.1145/985692.985722) | 採用: 優先順は固定・連打で巡回・履歴つき |
| 4 | ミニマップ + ping | 系譜の帯・レールのドット・statusbar ロールアップ・Fleet セグメントのバッジ（既） | 賛: 1 機 1 アイコンで検知 3 分 → 5 秒 [Porat](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290)。アニメは変化の見落としを減らすが同時多発は注意を薄める [NN/g](https://www.nngroup.com/articles/change-blindness-definition/)。否: コードに地理は無い | 翻案: 常時見える縮約 + 押すと飛ぶ + 一過性の ping。地図は作らない |
| 5 | 資源カウンタ（常時） | トークン / コスト（Task 行右端・statusbar Σ・既） | 賛: compute は現代の木・金・石 [Hoang](https://www.proofofconcept.pub/p/real-time-strategy-games-and-ai-interfaces)。FTL の動力バー = 有限予算の配分 [FTL](https://en.wikipedia.org/wiki/FTL:_Faster_Than_Light)。否: 得点化するとゲーミフィケーション [Moldon](https://johanneswachs.com/papers/msw_icse21.pdf) | 採用: ブリッジに Σ バー + Task ごとの context 残量 |
| 6 | 生産キュー / ビルドキュー | Captain の発令キュー（出した指示・待っている相手・depend） | 賛: Gas Town の不満は「なぜ止まったか見えない」[Tenzin](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)。状況認識 L3 [Endsley](https://www.researchgate.net/publication/210198492_Endsley_MR_Toward_a_Theory_of_Situation_Awareness_in_Dynamic_Systems_Human_Factors_Journal_371_32-64) | 採用（新規） |
| 7 | ラリーポイント | 完了後の既定の行き先 | 写像が弱い。phase の既定遷移で足りる | 回避 |
| 8 | 戦場の霧 | 未確認（done-unread）と digest の鮮度 | 賛: 古い情報を古いと示せば過信を防げる [Grunde-McLaughlin](https://arxiv.org/abs/2602.16844)。否: 霧マップは repo の地形化が前提 [AgentCraft](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration) | 翻案: 鮮度表示としてだけ |
| 9 | ヒーロー / 司令官ユニット | Captain（⌘0・ブリッジ・既） | 賛: Gas Town で最も評価が高いのは単一の窓口 [Appleton](https://maggieappleton.com/gastown)。群と話せば負担は O(1) [Lewis](https://journals.sagepub.com/doi/10.1177/1557234X13506688)。否: 擬人化は偽の社会的期待 [Nielsen](https://www.uxtigers.com/post/metaphor) | 採用: キャラクター化はしない |
| 10 | テックツリー | 系譜ツリー + Task 間の depend 辺 | 賛: 依存と機会費用が見える [metavert（意見）](https://metavert.io/tech-trees-in-games)。20 ノード以下は node-link が読める [Ghoniem](https://journals.sagepub.com/doi/10.1057/palgrave.ivs.9500092)。否: 直接の実証なし | 翻案: 語は「系譜」のまま |
| 11 | リプレイ / タイムライン | 事後タイムライン（采配ログ + 台帳） | 賛: 理解 38% 速く・正答 80 → 93% [AgentGUI](https://arxiv.org/abs/2607.26300)。AAR の 4 問 [AAR](https://en.wikipedia.org/wiki/After-action_review)。否: N=8 の単一研究 | 採用（新規・後段） |
| 12 | Strategic zoom / Sensors Manager | 舞台 1/2/3 列 ⇄ ブリッジの編隊図（既） | 賛: overview first [Shneiderman](https://www.cs.umd.edu/~ben/papers/Shneiderman1996eyes.pdf)。縮小時は形のアイコンへ [SupCom](https://en.wikipedia.org/wiki/Supreme_Commander_(video_game)) | 採用: 全段で同じグリフ語彙 |
| 13 | ポーズ可能なリアルタイム | 艦隊の一時停止（新規発令を止める） | 賛: 稼働率 70% 超で性能低下 [Cummings](https://link.springer.com/chapter/10.1007/978-3-540-72696-8_2)。昼には惰性で承認 [Osmani](https://addyosmani.com/blog/cognitive-parallel-agents/)。否: 類推のみ | 翻案（要検討） |
| 14 | FTL の船内図（部屋とクルー） | worktree = 部屋の絵 | 否: 一覧より情報が増えない。別の物に見せる装飾 [Few](https://www.perceptualedge.com/articles/Whitepapers/Common_Pitfalls.pdf) | 回避 |
| 15 | EVE: watch list・型付き broadcast・受信フィルタ・履歴 | ピン・要対応カード・ニュース（既） | 賛: 少数固定の要請語彙 + 一覧から直接操作 + 種別ごとの受信設定 [EVE Uni](https://wiki.eveuniversity.org/Fleet_interface) | 採用: 種別ごとの通知設定を足す |
| 16 | RimWorld: コロニストバー + アラート一覧 | ユニットフレーム列 + 要対応 | 賛: 一瞥要素はテキストでなく視覚で [Moon](https://www.moonlitdevelopment.com/development-blog/2017/12/25/the-designers-folder-rimworlds-at-a-glance-user-interface-elements)。「全員が赤」を「いま要る 1 人」に畳む [Radius UI](https://steamcommunity.com/sharedfiles/filedetails/?id=3788278207) | 採用 |
| 17 | Factorio: アラート一覧（分類・ピン） | 要対応の束ね | 賛: 10 分に 10 件超は洪水 [ISA-18.2](https://www.chemengonline.com/alarm-management-numbers/)。否: 出典は検索要約のみ [Factorio Wiki](https://stable.wiki.factorio.com/Alerts) | 翻案 |
| 18 | Football Manager: 責任分担 | Captain 権限表（FLEET-V2 §5.2）を画面に | 賛: 委任の境界が一覧で見える [GameRant](https://gamerant.com/football-manager-26-fm26-how-delegate-staff-responsibilities/)。consent が最良 [Ruff](https://direct.mit.edu/pvar/article-abstract/11/4/335/18417/Human-Interaction-with-Levels-of-Automation-and)。否: Integrate と承認は不変 | 翻案: 読み取り専用の一覧 |
| 19 | レイドフレーム（1 人 1 タイル・棒・枠・減光） | ユニットフレーム（編隊図のノード / Task 行） | 賛: 標準 UI は情報不足 78.4% / 68.4%・raid mod は最多利用 [Game Studies](https://gamestudies.org/1202/articles/ui_mod_in_wow)。状態の段階化で点検 5 分 → 2 分 [Porat](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290)。否: 設計原則の出典はフォーラム止まり | 採用 |
| 20 | Grid: status → indicator の割当 | 状態 → タイルの固定スロット（登録式） | 賛: 登録式の原則と一致 [Grid wiki](https://github.com/phanx-wow/Grid/wiki/FAQ)。個別選択を好む 9 割 [Game Studies](https://gamestudies.org/1202/articles/ui_mod_in_wow) | 翻案: まず固定・設定化は後 |
| 21 | 距離外の減光 | 注意不要の Task を減光 | 賛: 輝度は色相と別の前注意特徴 [Healey](https://www.csc2.ncsu.edu/faculty/healey/PP/)。フェードは「動き」に当たらない [WCAG 2.3.3](https://www.w3.org/WAI/WCAG21/Understanding/animation-from-interactions.html) | 採用 |
| 22 | fan-out・neglect time・interaction time | 放置できる時間と介入 1 回の時間 | 【確立】ただし設計者の目安 [Goodrich & Schultz](https://faculty.cs.byu.edu/~mike/mikeg/papers/HRISurvey.pdf) | 採用: 設計指標として |
| 23 | 稼働率 70%・待ち時間 | 同時 Task 数の目安・Blocked の経過表示（既） | 賛: [Cummings](https://link.springer.com/chapter/10.1007/978-3-540-72696-8_2)。実務家の上限 3〜4 [Osmani](https://addyosmani.com/blog/cognitive-parallel-agents/)。否: UAV からの転用は未検証 | 翻案 |
| 24 | management-by-consent / exception | Captain の推薦 + 人間の承認（既）/ 承認ポリシー | 賛: consent が最良 [Ruff](https://direct.mit.edu/pvar/article-abstract/11/4/335/18417/Human-Interaction-with-Levels-of-Automation-and)。consent 4–5 機・exception 8–12 機 [Porat](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290) | 採用: 現行を支持 |
| 25 | 状況認識 L1–L3 | L1 グリフ / L2 digest / L3 発令キュー | 【確立】[Endsley](https://www.researchgate.net/publication/210198492_Endsley_MR_Toward_a_Theory_of_Situation_Awareness_in_Dynamic_Systems_Human_Factors_Journal_371_32-64) | 採用 |
| 26 | アラーム管理・SRE の呼び出し規律 | エスカレーション段階表 | 【確立】[Chemical Engineering](https://www.chemengonline.com/alarm-management-numbers/) [SRE](https://sre.google/sre-book/monitoring-distributed-systems/) | 採用 |
| 27 | dark cockpit | 正常は静か（レールでは既） | 【実務知・二次資料】[Airflow](https://www.airflow.blog/2025/01/16/the-dark-cockpit-philosophy-enhancing-efficiency-and-safety-in-modern-aviation/) | 採用: 編隊図とサイドバーにも広げる |
| 28 | 前注意特徴・色相 → 形の干渉 | 状態グリフの色 | 【確立】[Healey](https://www.csc2.ncsu.edu/faculty/healey/PP/) | 翻案: 概観面では状態グリフを中立色に分ける（要検証） |
| 29 | 変化盲 | 状態遷移の見せ方・ニュース | 【確立】[Healey](https://www.csc2.ncsu.edu/faculty/healey/PP/) [NN/g](https://www.nngroup.com/articles/change-blindness-definition/) | 採用: 遷移は短く 1 箇所・取りこぼしはニュースで拾う |
| 30 | 紙のフライトストリップ | 要対応カードの手動の並べ替え / ピン | 【確立だが転用は弱い】[Mackay](https://www.lri.fr/~mackay/pdffiles/TOCHI2000.SaferPaper.pdf) | 翻案（優先度低） |
| 31 | ゲームの衣装（ピクセルオフィス・WC3 スキン・3D マップ・歩くキャラ） | — | 否: 翻訳税 [HN](https://news.ycombinator.com/item?id=46458936)・檻になる比喩 [Nielsen](https://www.uxtigers.com/post/metaphor)・装飾 [Few](https://www.perceptualedge.com/articles/Whitepapers/Common_Pitfalls.pdf)・比喩の不一致 [Bullock](https://arxiv.org/abs/2603.27908)・常時描画 [VS Code](https://github.com/microsoft/vscode/issues/22900)。賛: 需要はある（9.3k stars）[Pixel Agents](https://github.com/pablodelucca/pixel-agents) | 回避（本体では） |
| 32 | ゲーミフィケーション（スコア・連続記録・HP を得点に） | — | 否: 行動を歪めた [Moldon](https://johanneswachs.com/papers/msw_icse21.pdf)。[Gartner 2012](https://techcrunch.com/2012/11/27/badges-beware-80-of-gamification-apps-will-end-up-being-losers-says-gartner) | 回避 |
| 33 | 効果音（「基地が攻撃されています」） | 任意の通知音 | 賛: Pixel Agents と AgentCraft が採用。否: 効果の実証なし | 翻案: 既定オフ・窓が非アクティブの時だけ |

#### 成果物 2: ブリッジと Captain のための HUD 案（8 つ）

**1. ユニットフレーム — 編隊図のノードを「1 Task = 1 タイル」にする**
- 中身（スロットは固定）: 状態グリフ / 名前 + ⎇ branch / context 使用量の棒 / 実行中のツール + 経過 / 要対応マーカー + 経過 / `+N −M` / ⚑（Captain が起動）。
- 根拠: 1 機 1 アイコンで検知が 3 分 → 5 秒（[Porat](https://pmc.ncbi.nlm.nih.gov/articles/PMC4878290)）。レイド系 mod は最多利用カテゴリ（[Game Studies](https://gamestudies.org/1202/articles/ui_mod_in_wow)）。一瞥要素は視覚で（[Moon](https://www.moonlitdevelopment.com/development-blog/2017/12/25/the-designers-folder-rimworlds-at-a-glance-user-interface-elements)）。ハブ & スポークは星型で、位相としては「所属」以上の情報を持たない。だから情報はノードに載せる。node-link が読めるのは 20 ノード以下（[Ghoniem](https://journals.sagepub.com/doi/10.1057/palgrave.ivs.9500092)）で、necoder の Task 数はその内側。
- 掟との整合: 棒はスレッド色（UI-SPEC §1.3 でトークンメーターは許可済み）。閾値は色を変えず目盛りと形で。タイルの位置は固定し、並び替わるのは要対応キューだけ。N ≤ 5 は現行の 3 段、N ≥ 10 で 1 段に落とす（VuhDo の 5 / 10 / 25 プリセットに倣う・[Healing Spec](https://healingspec.com/2012/04/10/188/)）。目立つ動きは Blocked だけ。

**2. 「次に自分が要る所」キーの意味論と、番号の固定割当**
- ⌘⇧U = 要対応の先頭へ。順は FLEET-V2 §3.2 のまま（Blocked 経過順 → Failed → レビュー待ち → 完了・未確認）。連打で巡回、直近 8 件ほどの履歴を戻れる。0 件なら何もしない + 短い表示。⌘1..9 は Task に固定で割り当て、2 度押しで composer にフォーカス。「手空き N」は Captain バーに件数だけ。
- 根拠: StarCraft II の Space（履歴 8 件）・F1・Ctrl+数字・2 度押し（[Blizzard](https://news.blizzard.com/en-us/article/4552955/game-guide-special-control)）。AgentCraft の Space（[GitNation](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration)）。interaction time を縮めれば fan-out が上がる（[Olsen & Wood](https://dl.acm.org/doi/10.1145/985692.985722)）。
- 掟との整合: 行き先名のフラッシュは既存のレール切替と同じ流儀で中立色。キーは母語 Zed 互換の枠内。

**3. エスカレーション段階表（アラートのはしご）**
- L0 Working: 静か（概観面では低振幅か静止）→ L1 Done: グリフが変わる + ニュース 1 行 → L2 Blocked: 要対応カード + Fleet セグメントのバッジ + レールのドット → L3 15 秒超: 脈動を速く + Captain の推薦（既存）→ L4 数分超、または窓が非アクティブ: OS 通知（音は既定オフ）。洪水対策として、同種の承認は 1 枚に束ね、10 分に 10 件を超えたら「N 件まとめて」へ畳む。
- 根拠: 6〜12 件/時が上限・10 分に 10 件超は洪水（[Chemical Engineering](https://www.chemengonline.com/alarm-management-numbers/)）。"Every page should be actionable"（[SRE](https://sre.google/sre-book/monitoring-distributed-systems/)）。dark cockpit（[Airflow](https://www.airflow.blog/2025/01/16/the-dark-cockpit-philosophy-enhancing-efficiency-and-safety-in-modern-aviation/)）。競合するアニメは注意を薄める（[NN/g](https://www.nngroup.com/articles/animation-purpose-ux/)）。「全員が赤」を 1 件に畳む（[Radius UI](https://steamcommunity.com/sharedfiles/filedetails/?id=3788278207)）。
- 掟との整合: 色相は使わない（件数バッジの err 色ボーダーは FLEET-V2 §3.0 の既存判断の範囲）。5 秒を超える脈動は数回で止め、形と位置を残す（[WCAG 2.2.2](https://www.w3.org/WAI/WCAG21/Understanding/pause-stop-hide.html)）。`reduce_motion` でも全段が形だけで読めること。

**4. 資源バー — 艦隊の Σ と Task ごとの context 残量**
- ブリッジの見出しに Σ トークン（分かれば金額）と Captain 自身の消費。ユニットフレームに context 使用量の棒。
- 根拠: RTS の資源カウンタの写像（[Hoang](https://www.proofofconcept.pub/p/real-time-strategy-games-and-ai-interfaces)）。FTL の動力配分（[Wikipedia](https://en.wikipedia.org/wiki/FTL:_Faster_Than_Light)）。Gas Town への不満の柱はコストの見えなさ（[Tenzin](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)・[HN](https://news.ycombinator.com/item?id=46458936)）。necoder の原点の痛点でもある（トークン常時表示・FLEET-V2 §5.6「見えないコストを作らない」）。
- 掟との整合: 棒の色はスレッド色。満杯側の警告は目盛り・形・数字の太さで。順位・得点・達成表示にはしない（[Moldon](https://johanneswachs.com/papers/msw_icse21.pdf)）。

**5. 発令キュー — Captain が何を出し、何を待っているか**
- ブリッジのサイドペインに「発令 → 稼働中 → 待ち（誰の何を）→ 報告済み」の列。Captain の `wait` / `depend` / `wait-deps`（FLEET-V2 §5.2）をそのまま描く。采配ログ（過去）と対になる「これから」の面。
- 根拠: RTS の生産キュー。Gas Town をやめた人の具体的な不満は「なぜ止まったか分からない」（[Tenzin](https://tenzinwangdhen.com/posts/gastown-good-bad-ugly/)）。状況認識の L3 = 予測（[Endsley](https://www.researchgate.net/publication/210198492_Endsley_MR_Toward_a_Theory_of_Situation_Awareness_in_Dynamic_Systems_Human_Factors_Journal_371_32-64)）。自動化は人間を輪の外に出す（[Endsley & Kiris](https://journals.sagepub.com/doi/10.1518/001872095779064555)）。委任の中身が一覧で見える（[Football Manager](https://gamerant.com/football-manager-26-fm26-how-delegate-staff-responsibilities/)）。
- 掟との整合: 文字と中立色の行。Task 名の前の ● だけ識別色。

**6. 系譜 = 依存つきのツリー。ハブ図はユニットフレームの配置に**
- 「ツリー」表示に Task 間の depend 辺と、統合済み / 分岐中の区別を載せる。「ハブ」表示は位相の情報が無いので、案 1 のタイルを Captain の周りに固定配置する面と割り切る。
- 根拠: 小さく疎なグラフは node-link が読める（[Ghoniem](https://journals.sagepub.com/doi/10.1057/palgrave.ivs.9500092)）。テックツリーは依存と順序を見せる（意見: [metavert](https://metavert.io/tech-trees-in-games)）。overview first（[Shneiderman](https://www.cs.umd.edu/~ben/papers/Shneiderman1996eyes.pdf)）。
- 掟との整合: 線は Task 色（識別）。状態は線種と端の形（実線 / 破線・`╰━`）。UI の語は「系譜」のままで、「テックツリー」は設計語に留める。

**7. 事後タイムライン（after-action）**
- 目標 1 つにつき「頼んだこと → Captain が切った Task → 各 Task の経過（承認待ちの長さ・失敗・介入）→ 統合」を 1 本の時間軸で。AAR の 4 問に答える形。台帳とニュースが既にデータを持っている。
- 根拠: タイムラインと activity feed で理解が 38% 速く・正答 80 → 93%（[AgentGUI](https://arxiv.org/abs/2607.26300)）。AAR（[Wikipedia](https://en.wikipedia.org/wiki/After-action_review)）。Sheridan の 5 機能のうち learning に当たる面が今は無い（[Supervisory control](https://en.wikipedia.org/wiki/Supervisory_control)）。
- 掟との整合と注意: 帯は Task 色・状態は形。要約の隣に必ず diff と実行ログへの 1 クリックを置く。良い可視化は自信だけを上げうる（[Grunde-McLaughlin](https://arxiv.org/abs/2602.16844)）。

**8. 状態の符号化の見直し（掟の内側での調整）**
- (a) 概観面（編隊図・非フォーカス時のサイドバー）では、状態グリフを中立色で識別色ドットの隣に分けて描く。(b) 目立つ動きは Blocked の 1 クラスだけにし、Working は静かに。(c) 注意不要の Task は減光。(d) `reduce_motion` 下で 5 状態が形だけで区別できるかを確かめる。
- 根拠: 色相のばらつきは形の読み取りを妨げ、逆は起きない。周辺視の flicker 弁別には 100% 以上の差が要る（[Healey](https://www.csc2.ncsu.edu/faculty/healey/PP/)）。Fortnite は赤を 1 つの意味に取っておいて見落としを減らした（[Hodent](https://medium.com/ironsource-levelup/understanding-the-success-of-fortnite-a-ux-and-psychology-perspective-part-2-eea824ae63ca)）。アイコンは誤読されるのでテストする（[Hodent](https://celiahodent.com/understanding-the-success-of-fortnite-ux/)）。
- 位置づけ: これは「状態に色相を使わない」掟を強める提案で、破る提案ではない。ただし UI-SPEC §6 の「状態ドットは常にスレッド識別色」という明示判断に触れるので、まず実測（Working 7 + Blocked 1 の画面で、composer を見ながら気づくまでの秒数を、現行と中立色案で比べる）で決めるのがよい。

#### 成果物 3: 「ゲームアナロジーは良い案か」への答え

**答え: 機能語彙を借りるのは良い案。製品をゲームとして着飾るのは悪い案。necoder は既に前者の側にいて、その判断は根拠に支えられている。**

理由は 3 つ。
1. RTS / 艦隊 / レイドの HUD は「1 人の注意で多数の半自律ユニットを回す」問題を何十年も解いてきた。その解（1 キーで要対応へ・動かない番号・1 体 1 タイル・常時見える資源・正常は静か）は、監督制御の人間工学が別経路で出した結論（interaction time を縮める・状況認識の 3 段を支える・行動できるアラートだけ出す）と同じ向きを向いている。2 つの独立した系譜が一致する装置は、借りて外れにくい。
2. 一方で、ゲームの見た目や世界観が監督の成績を上げたという証拠は見つからなかった。あるのは逆向きの材料: Gas Town の翻訳税、設計者とユーザーの比喩の不一致（CHI 2026）、開発者ツールのゲーミフィケーションが行動を歪めた自然実験（ICSE 2021）、装飾は目的の邪魔という実務知（Few）、常時アニメの性能コスト（VS Code）。ゲーム開発の第一線も「明快さのためなら格好いい案を捨てる」側に立っている。
3. necoder の掟（色は識別・装飾なし・性能予算）は、この切り分けとそのまま噛み合う。Fortnite が赤を 1 つの意味に取っておいたのと同じ構造の規律で、ゲーム UX の実例が掟を支持している。

**境界線の引き方**（迷ったら上から順に当てる）:
- その装置は、介入 1 回の時間を縮めるか、放置できる時間を延ばすか、状況認識の段（知覚・理解・予測）のどれかを上げるか。どれでもなければ装飾。
- 元のゲームを知らない人が、画面の文脈だけで使えるか。知らないと読めないなら衣装。
- UI 文字列に新しい名詞が増えるか。増えるなら、その語に既成の実物の名前が無い時だけ。
- 正常時に静かか。何も起きていない時に動く・光るものは外す。
- 止まっている時に再描画を要求しないか。要求するなら性能予算に反する。
- 得点・順位・達成・連続記録の形をしていないか。していたら外す。

**「もっとゲームのような明快さへ押すべきか」への具体的な答え**: 押す方向は「絵をゲームっぽく」ではなく「ノードを情報タイルに・番号を固定に・Captain の待ちを見える化」。編隊図が実機で畳まれたまま「ただのチャット」に見えていたという FLEET-V2 の記録は、図に情報が足りなかったことの表れと読める。ユニットフレームにすれば、編隊図は Fleet らしい絵であると同時に、開いておく理由のある概観になる。

**猫のマスコット**: ブランド・空状態・オンボーディング・About には置いてよい（Pixel Agents の人気が示す愛着の需要を安く取れる）。状態の伝達路にはしない。Captain をキャラクターにもしない。擬人化は偽の社会的期待を生み、外した時に見下されたように感じさせる。ピクセルオフィス的な面をどうしても試すなら、本体の外の opt-in（既定オフ）に限る。

### Gaps
- ここでの推奨の多くは、UAV / ロボット監督研究とゲーム実務知からの類推。**複数のコーディングエージェントを 1 人が監督する条件で、これらの装置の効果を直接測った研究は見つからなかった**。necoder 自身のドッグフーディングで測れる指標を持つのが現実的（例: Blocked になってから応答までの中央値 = 待ち時間、1 時間あたりの要対応件数、⌘⇧U の使用率、同時稼働 Task 数と承認の質）。
- 案 8（中立色の状態グリフ・動きは 1 クラス）は知覚研究からの推論で、necoder の実画面での検証が要る。
- 発令キューと事後タイムラインは、根拠の強さ（状況認識 L3・AgentGUI）に比べて実装コストの見積もりをしていない。
- 音の通知、艦隊の一時停止、承認ポリシー（management-by-exception）は、効果とリスクの両方について今回の根拠が薄い。
