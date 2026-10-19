necoder for Windows（ポータブル版 / portable）
================================================

■ 起動するには
  necoder.exe（猫のアイコン）をダブルクリックしてください。
  ほかのファイル（licenses フォルダ・control フォルダなど）は necoder.exe と同じ場所に置いたままにしてください。

■ スタートメニュー・デスクトップのショートカット・右クリックの「necoder で開く」が欲しい場合
  この zip ではなく、インストーラ版（necoder-windows-x64-setup.exe）を使ってください。
  管理者権限は要りません（%LOCALAPPDATA%\Programs\necoder に入ります）。
    https://github.com/iKora128/necoder/releases/latest

  PowerShell で次の 1 行を実行しても入れられます（「Windows によって PC が保護されました」の画面が出ません）:
    irm https://necoder.com/install.ps1 | iex

■ 「Windows によって PC が保護されました」（青い画面）が出たら
  necoder はまだコード署名をしていないため、ブラウザで落とした exe を初めて開くと出ます。
  「詳細情報」→「実行」で起動できます。2 回目からは出ません。

■ 設定と履歴の置き場
  設定: %APPDATA%\necoder　　データ（会話の履歴など）: %LOCALAPPDATA%\necoder
  necoder.exe を消しても、これらは残ります。


■ To start
  Double-click necoder.exe (the cat icon). Keep the other files next to it.

■ Want Start menu / desktop shortcuts and "Open with necoder" in Explorer?
  Use the installer (necoder-windows-x64-setup.exe) instead of this zip. No admin rights needed.
    https://github.com/iKora128/necoder/releases/latest
  Or run this in PowerShell (no "Windows protected your PC" screen):
    irm https://necoder.com/install.ps1 | iex

■ "Windows protected your PC" (blue screen)?
  necoder is not code-signed yet. Click "More info" → "Run anyway". It only appears the first time.
