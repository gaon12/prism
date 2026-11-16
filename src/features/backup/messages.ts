import type { Language } from '../../lib/types';
// Korean, English, Simplified Chinese, Traditional Chinese, Japanese.
const messages = {
  title: ['백업과 복원', 'Backup and restore', '备份与恢复', '備份與還原', 'バックアップと復元'],
  description: [
    '설정, 검색 폴더, 최근 검색, AI 대화와 문서를 고른 폴더에 복사해 둡니다. API 키는 이 컴퓨터의 키체인에 남고, 검색 색인과 클립보드 기록은 담지 않습니다(색인은 원본 파일에서 다시 만듭니다).',
    'Copy settings, search folders, recent searches, AI chats and documents into a folder you choose. API keys stay in this computer’s keychain; search indexes and clipboard history are not included (indexes are rebuilt from the original files).',
    '将设置、搜索文件夹、最近搜索、AI 对话和文档复制到你选择的文件夹。API 密钥保留在本机钥匙串中，不包含搜索索引和剪贴板记录（索引会从原始文件重新生成）。',
    '將設定、搜尋資料夾、最近搜尋、AI 對話與文件複製到你選擇的資料夾。API 金鑰保留在本機鑰匙圈中，不包含搜尋索引與剪貼簿記錄（索引會從原始檔案重新建立）。',
    '設定・検索フォルダー・最近の検索・AI チャット・文書を選んだフォルダーにコピーします。API キーはこのコンピューターのキーチェーンに残り、検索インデックスとクリップボード履歴は含みません（インデックスは元のファイルから再作成します）。',
  ],
  create: ['백업 만들기…', 'Back up…', '创建备份…', '建立備份…', 'バックアップ…'],
  restore: [
    '백업에서 복원…',
    'Restore from backup…',
    '从备份恢复…',
    '從備份還原…',
    'バックアップから復元…',
  ],
  created: [
    '백업을 만들었습니다: {path}',
    'Backup saved: {path}',
    '已创建备份：{path}',
    '已建立備份：{path}',
    'バックアップを作成しました: {path}',
  ],
  confirmTitle: [
    '백업에서 복원할까요?',
    'Restore from this backup?',
    '要从此备份恢复吗？',
    '要從此備份還原嗎？',
    'このバックアップから復元しますか？',
  ],
  confirmBody: [
    '지금의 설정, 대화와 문서가 백업 당시의 내용으로 바뀝니다. Prism이 다시 시작되며, 바뀌기 전 데이터는 데이터 폴더의 restore-previous에 한 벌 보관합니다.',
    'Your current settings, chats and documents are replaced with the backup’s. Prism restarts; the data it replaces is kept once in restore-previous in the data folder.',
    '当前的设置、对话和文档将替换为备份中的内容。Prism 会重新启动，被替换的数据会在数据文件夹的 restore-previous 中保留一份。',
    '目前的設定、對話與文件將換成備份中的內容。Prism 會重新啟動，被取代的資料會在資料夾的 restore-previous 中保留一份。',
    '現在の設定・チャット・文書がバックアップの内容に置き換わります。Prism は再起動し、置き換え前のデータはデータフォルダーの restore-previous に 1 つ保管します。',
  ],
  confirm: [
    '복원하고 다시 시작',
    'Restore and restart',
    '恢复并重启',
    '還原並重新啟動',
    '復元して再起動',
  ],
  cancel: ['취소', 'Cancel', '取消', '取消', 'キャンセル'],
  working: ['처리하는 중…', 'Working…', '处理中…', '處理中…', '処理中…'],
  pending: [
    '복원이 준비되어 있습니다. 다음에 Prism을 시작할 때 적용됩니다.',
    'A restore is ready and will be applied the next time Prism starts.',
    '恢复已准备好，将在下次启动 Prism 时应用。',
    '還原已準備好，將在下次啟動 Prism 時套用。',
    '復元の準備ができています。次に Prism を起動したときに適用されます。',
  ],
  cancelPending: ['복원 취소', 'Cancel restore', '取消恢复', '取消還原', '復元を取り消す'],
  restored: [
    '백업에서 복원한 데이터로 시작했습니다.',
    'Prism started with the restored backup.',
    'Prism 已使用恢复的备份启动。',
    'Prism 已使用還原的備份啟動。',
    '復元したバックアップで起動しました。',
  ],
  restoreFailed: [
    '백업을 복원하지 못해 기존 데이터로 시작했습니다.',
    'The backup could not be restored; Prism started with its existing data.',
    '无法恢复备份，Prism 已使用现有数据启动。',
    '無法還原備份，Prism 已使用現有資料啟動。',
    'バックアップを復元できなかったため、既存のデータで起動しました。',
  ],
  // The native layer reports these in English; show them in the person's language.
  notBackup: [
    '이 폴더는 완성된 Prism 백업이 아닙니다.',
    'This folder is not a finished Prism backup.',
    '此文件夹不是完整的 Prism 备份。',
    '此資料夾不是完整的 Prism 備份。',
    'このフォルダーは完成した Prism バックアップではありません。',
  ],
  newer: [
    '더 새로운 Prism에서 만든 백업입니다. 먼저 Prism을 업데이트해 주세요.',
    'This backup was made by a newer Prism. Update Prism first.',
    '此备份由更新版本的 Prism 创建，请先更新 Prism。',
    '此備份由較新版本的 Prism 建立，請先更新 Prism。',
    'より新しい Prism で作成されたバックアップです。先に Prism を更新してください。',
  ],
  damaged: [
    '백업 파일이 손상되었거나 빠져 있습니다.',
    'A file in the backup is damaged or missing.',
    '备份中的文件已损坏或缺失。',
    '備份中的檔案已損毀或遺失。',
    'バックアップ内のファイルが壊れているか、見つかりません。',
  ],
  failed: [
    '백업 작업에 실패했습니다: {reason}',
    'The backup operation failed: {reason}',
    '备份操作失败：{reason}',
    '備份作業失敗：{reason}',
    'バックアップ操作に失敗しました: {reason}',
  ],
} satisfies Record<string, readonly [string, string, string, string, string]>;
const indexes: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export type BackupMessage = keyof typeof messages;
export const backupText = (
  language: Language,
  key: BackupMessage,
  values: Record<string, string> = {},
) =>
  Object.entries(values).reduce(
    (text, [name, value]) => text.replaceAll(`{${name}}`, value),
    messages[key][indexes[language]] as string,
  );
/** Turns a native backup error (English) into a localized sentence. */
export function backupFailure(language: Language, reason: string) {
  if (reason.includes('not a finished Prism backup')) return backupText(language, 'notBackup');
  if (reason.includes('newer Prism')) return backupText(language, 'newer');
  if (/damaged|missing|unknown file/.test(reason)) return backupText(language, 'damaged');
  return backupText(language, 'failed', { reason });
}
