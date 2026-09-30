//! エクスプローラのファイル操作の取り消し（H30・local のみ）。
//!
//! 作成・名前変更・移動・複製・ゴミ箱を、⌘Z（エクスプローラにフォーカスがある時だけ）で
//! 新しい順に 1 手ずつ戻す。記録するのは「何をどこへ動かしたか」だけで、中身は持たない。
//! 戻す時も**上書きしない**（戻し先に同名があれば断る）し、**中身のあるものを完全には消さない**
//! （作成を戻す時、空でなければゴミ箱へ入れる）。
//!
//! 手法の参考: stablyai/orca@646e9a5 の
//! `src/renderer/src/components/right-sidebar/fileExplorerUndoRedo.ts`（線形の履歴・上限 50 手）。
//! Orca はゴミ箱からの復元を持たないが、necoder は macOS の `/usr/bin/trash -v` が出す
//! 「ゴミ箱の中での場所」を覚えて戻す（[`crate::move_to_trash_local`]）。

use std::path::{Path, PathBuf};

/// エクスプローラのファイル操作 1 つ（取り消すための材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOperation {
    /// 新規ファイル・新規フォルダ・複製・Finder からのコピーで `path` ができた。
    Created { path: PathBuf },
    /// 名前変更・エクスプローラ内の移動。
    Moved { from: PathBuf, to: PathBuf },
    /// ゴミ箱へ入れた。`trashed` はゴミ箱の中での場所（分からなければ `None` = 戻せない）。
    Trashed {
        original: PathBuf,
        trashed: Option<PathBuf>,
    },
}

/// 取り消せなかった理由。文言は UI 側で i18n する（この crate は i18n を知らない）。
#[derive(Debug)]
pub enum UndoError {
    /// ゴミ箱のどこへ入ったか分からない（Finder 経由の旧経路・macOS 以外）。
    TrashLocationUnknown { original: PathBuf },
    /// ゴミ箱から無くなっている（ゴミ箱を空にした・Finder の「戻す」を使った）。
    MissingFromTrash { original: PathBuf },
    /// 戻す先に同じ名前がある（上書きしない）。
    DestinationExists { path: PathBuf },
    /// 戻す対象がもう無い（外で消された・動かされた）。
    SourceMissing { path: PathBuf },
    /// ファイルシステムの失敗（権限・親フォルダが無い 等）。
    Io(anyhow::Error),
}

impl std::fmt::Display for UndoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UndoError::TrashLocationUnknown { original } => write!(
                formatter,
                "ゴミ箱の中の場所が分からない: {}",
                original.display()
            ),
            UndoError::MissingFromTrash { original } => {
                write!(formatter, "ゴミ箱に見つからない: {}", original.display())
            }
            UndoError::DestinationExists { path } => {
                write!(formatter, "戻し先が既に存在する: {}", path.display())
            }
            UndoError::SourceMissing { path } => {
                write!(formatter, "戻す対象が見つからない: {}", path.display())
            }
            UndoError::Io(error) => write!(formatter, "{error:#}"),
        }
    }
}

/// 取り消しの履歴。1 回のユーザー操作 = 1 手（複数ファイルのドロップも 1 手）。新しい手が末尾。
#[derive(Debug, Default)]
pub struct FileOperationHistory {
    steps: Vec<Vec<FileOperation>>,
}

impl FileOperationHistory {
    /// 覚えておく手数の上限。超えたら古い手から捨てる。
    pub const LIMIT: usize = 50;

    /// 1 手を記録する（空なら何もしない）。
    pub fn record(&mut self, operations: Vec<FileOperation>) {
        if operations.is_empty() {
            return;
        }
        self.steps.push(operations);
        if self.steps.len() > Self::LIMIT {
            self.steps.remove(0);
        }
    }

    /// いちばん新しい手を取り出す（取り消しに成功しても失敗しても、同じ手は二度戻さない）。
    pub fn pop(&mut self) -> Option<Vec<FileOperation>> {
        self.steps.pop()
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// 1 手（複数の操作）を後ろから順に取り消す。途中で失敗したらそこで止める
/// （それより前の操作は戻したまま＝どこまで戻ったかはファイルの状態がそのまま示す）。
pub fn undo_file_operations_local(operations: &[FileOperation]) -> Result<(), UndoError> {
    for operation in operations.iter().rev() {
        undo_file_operation_local(operation)?;
    }
    Ok(())
}

/// 操作 1 つを取り消す。
pub fn undo_file_operation_local(operation: &FileOperation) -> Result<(), UndoError> {
    match operation {
        FileOperation::Created { path } => remove_created(path),
        FileOperation::Moved { from, to } => move_back(to, from),
        FileOperation::Trashed { original, trashed } => {
            let Some(location) = trashed else {
                return Err(UndoError::TrashLocationUnknown {
                    original: original.clone(),
                });
            };
            if location.symlink_metadata().is_err() {
                return Err(UndoError::MissingFromTrash {
                    original: original.clone(),
                });
            }
            move_back(location, original)
        }
    }
}

/// 作ったものを片付ける。空のファイル・空のフォルダは消す（失うものが無い）。中身があれば
/// ゴミ箱へ（作った後に書き込んだ内容を、取り消し 1 回で完全に失わせない）。
fn remove_created(path: &Path) -> Result<(), UndoError> {
    let Ok(metadata) = path.symlink_metadata() else {
        return Err(UndoError::SourceMissing {
            path: path.to_path_buf(),
        });
    };
    let removed = if metadata.is_file() && metadata.len() == 0 {
        std::fs::remove_file(path).map_err(anyhow::Error::from)
    } else if metadata.is_dir() && directory_is_empty(path) {
        std::fs::remove_dir(path).map_err(anyhow::Error::from)
    } else {
        crate::move_to_trash_local(path).map(|_location| ())
    };
    removed
        .map_err(|error| UndoError::Io(error.context(format!("片付けに失敗: {}", path.display()))))
}

fn directory_is_empty(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// `current` を `destination` へ戻す（上書きしない）。
fn move_back(current: &Path, destination: &Path) -> Result<(), UndoError> {
    if current.symlink_metadata().is_err() {
        return Err(UndoError::SourceMissing {
            path: current.to_path_buf(),
        });
    }
    if destination.symlink_metadata().is_ok() {
        return Err(UndoError::DestinationExists {
            path: destination.to_path_buf(),
        });
    }
    std::fs::rename(current, destination).map_err(|error| {
        UndoError::Io(anyhow::Error::from(error).context(format!(
            "戻せない: {} → {}",
            current.display(),
            destination.display()
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "necoder_file_operations_{tag}_{}",
            std::process::id()
        ));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).expect("前回の一時ディレクトリを消す");
        }
        std::fs::create_dir_all(&directory).expect("一時ディレクトリを作る");
        directory
    }

    #[test]
    fn history_pops_newest_first_and_forgets_beyond_the_limit() {
        let mut history = FileOperationHistory::default();
        history.record(Vec::new());
        assert!(history.is_empty(), "空の手は記録しない");
        for index in 0..FileOperationHistory::LIMIT + 5 {
            history.record(vec![FileOperation::Created {
                path: PathBuf::from(format!("/tmp/{index}")),
            }]);
        }
        assert_eq!(history.len(), FileOperationHistory::LIMIT);
        assert_eq!(
            history.pop(),
            Some(vec![FileOperation::Created {
                path: PathBuf::from(format!("/tmp/{}", FileOperationHistory::LIMIT + 4)),
            }]),
            "新しい手から戻す"
        );
    }

    #[test]
    fn undoing_a_create_removes_the_new_empty_file_and_folder() {
        let root = scratch("create");
        let file = root.join("new.rs");
        let folder = root.join("new-folder");
        crate::create_file_local(&file).expect("ファイルを作る");
        crate::create_dir_local(&folder).expect("フォルダを作る");
        let mut history = FileOperationHistory::default();
        history.record(vec![FileOperation::Created { path: file.clone() }]);
        history.record(vec![FileOperation::Created {
            path: folder.clone(),
        }]);

        let step = history.pop().expect("フォルダ作成の手");
        undo_file_operations_local(&step).expect("フォルダ作成を戻す");
        assert!(!folder.exists(), "空のフォルダは消える");
        assert!(file.exists(), "1 手ずつ戻す（ファイルはまだ残る）");

        let step = history.pop().expect("ファイル作成の手");
        undo_file_operations_local(&step).expect("ファイル作成を戻す");
        assert!(!file.exists(), "空のファイルは消える");
        assert!(history.pop().is_none());

        std::fs::remove_dir_all(&root).expect("後片付け");
    }

    #[test]
    fn undoing_a_rename_puts_the_old_name_back_without_overwriting() {
        let root = scratch("rename");
        let before = root.join("before.txt");
        let after = root.join("after.txt");
        std::fs::write(&before, "中身").expect("元のファイル");
        crate::rename_local(&before, &after).expect("名前を変える");
        let operation = FileOperation::Moved {
            from: before.clone(),
            to: after.clone(),
        };

        // 戻し先に同名ができていたら断り、どちらも触らない。
        std::fs::write(&before, "別物").expect("同名を置く");
        assert!(matches!(
            undo_file_operation_local(&operation),
            Err(UndoError::DestinationExists { .. })
        ));
        assert_eq!(std::fs::read_to_string(&before).expect("読む"), "別物");
        assert!(after.exists());

        std::fs::remove_file(&before).expect("同名を消す");
        undo_file_operation_local(&operation).expect("名前の変更を戻す");
        assert_eq!(std::fs::read_to_string(&before).expect("読む"), "中身");
        assert!(!after.exists());

        // 対象がもう無い（外で消された）なら SourceMissing。
        assert!(matches!(
            undo_file_operation_local(&operation),
            Err(UndoError::SourceMissing { .. })
        ));

        std::fs::remove_dir_all(&root).expect("後片付け");
    }

    #[test]
    fn undoing_a_move_and_a_duplicate() {
        let root = scratch("move");
        let source = root.join("a.txt");
        let target_dir = root.join("sub");
        std::fs::create_dir_all(&target_dir).expect("移動先");
        std::fs::write(&source, "a").expect("元");
        let moved = target_dir.join("a.txt");
        crate::rename_local(&source, &moved).expect("移動");
        let copy = crate::duplicate_local(&moved).expect("複製");
        std::fs::write(&copy, "").expect("複製を空にして消える側の分岐を通す");

        undo_file_operations_local(&[
            FileOperation::Moved {
                from: source.clone(),
                to: moved.clone(),
            },
            FileOperation::Created { path: copy.clone() },
        ])
        .expect("後ろから順に戻す");
        assert!(!copy.exists(), "複製は消える");
        assert!(source.exists(), "移動は元の場所へ戻る");
        assert!(!moved.exists());

        std::fs::remove_dir_all(&root).expect("後片付け");
    }

    /// 本物のゴミ箱には触れず、「ゴミ箱の中での場所」を一時フォルダで代用して戻し方を確かめる。
    #[test]
    fn undoing_a_trash_restores_from_the_recorded_location() {
        let root = scratch("trash");
        let fake_trash = root.join(".Trash");
        std::fs::create_dir_all(&fake_trash).expect("代用のゴミ箱");
        let original = root.join("notes.md");
        let trashed = fake_trash.join("notes 2.md");
        std::fs::write(&trashed, "残したい").expect("ゴミ箱の中の項目");

        let operation = FileOperation::Trashed {
            original: original.clone(),
            trashed: Some(trashed.clone()),
        };
        undo_file_operation_local(&operation).expect("ゴミ箱から戻す");
        assert_eq!(
            std::fs::read_to_string(&original).expect("読む"),
            "残したい"
        );
        assert!(!trashed.exists());

        // もう一度 = ゴミ箱に無い（Finder で戻した・空にした）。
        std::fs::remove_file(&original).expect("元を消す");
        assert!(matches!(
            undo_file_operation_local(&operation),
            Err(UndoError::MissingFromTrash { .. })
        ));
        // 場所が分からない = 戻せない。
        assert!(matches!(
            undo_file_operation_local(&FileOperation::Trashed {
                original,
                trashed: None,
            }),
            Err(UndoError::TrashLocationUnknown { .. })
        ));

        std::fs::remove_dir_all(&root).expect("後片付け");
    }
}
