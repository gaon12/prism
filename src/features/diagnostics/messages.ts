import type { Language } from '../../lib/types';

const messages = {
  title: ['오류 보고', 'Report a problem', '报告问题', '回報問題', '問題を報告'],
  description: [
    '문제가 생긴 동작과 재현 순서',
    'What happened and how to reproduce it',
    '发生的问题及重现步骤',
    '發生的問題及重現步驟',
    '問題の内容と再現手順',
  ],
  privacy: [
    '진단에는 앱·운영체제 정보, 기능 상태와 개수가 포함됩니다. 문서 내용, 파일 경로, 검색어, 클립보드 내용, API 키와 원문 오류는 수집하지 않습니다. 직접 적은 설명은 보고서에 포함됩니다.',
    'Diagnostics include app and OS information, feature status and counts. Document text, file paths, searches, clipboard contents, API keys and raw errors are excluded. Notes you type are included in the report.',
    '诊断包含应用和操作系统信息、功能状态及数量。不收集文档内容、文件路径、搜索词、剪贴板内容、API密钥或原始错误。您填写的说明会包含在报告中。',
    '診斷包含應用程式和作業系統資訊、功能狀態及數量。不收集文件內容、檔案路徑、搜尋字詞、剪貼簿內容、API金鑰或原始錯誤。您填寫的說明會包含在報告中。',
    '診断にはアプリとOSの情報、機能の状態と件数が含まれます。文書の内容、ファイルパス、検索語、クリップボードの内容、APIキー、エラー原文は収集しません。入力した説明はレポートに含まれます。',
  ],
  capture: ['진단 미리보기', 'Preview diagnostics', '预览诊断', '預覽診斷', '診断をプレビュー'],
  refresh: [
    '진단 새로 수집',
    'Refresh diagnostics',
    '重新收集诊断',
    '重新收集診斷',
    '診断を再取得',
  ],
  preview: ['보고서 미리보기', 'Report preview', '报告预览', '報告預覽', 'レポートのプレビュー'],
  review: [
    '아래 내용만 복사하거나 저장합니다. 개인정보를 적지 않았는지 확인하세요. 자동으로 전송하지 않습니다.',
    'Only the text below is copied or saved. Check your notes for personal information. Nothing is sent automatically.',
    '仅复制或保存以下内容。请检查说明中是否包含个人信息。不会自动发送。',
    '僅複製或儲存以下內容。請檢查說明中是否包含個人資訊。不會自動傳送。',
    '以下の内容のみコピー・保存します。説明に個人情報がないか確認してください。自動送信はしません。',
  ],
  copy: ['보고서 복사', 'Copy report', '复制报告', '複製報告', 'レポートをコピー'],
  save: ['파일로 저장', 'Save to file', '保存到文件', '儲存至檔案', 'ファイルに保存'],
  issue: [
    'GitHub에서 문제 보고',
    'Report on GitHub',
    '在GitHub上报告问题',
    '在GitHub上回報問題',
    'GitHubで問題を報告',
  ],
  public: [
    'GitHub 이슈는 공개됩니다. 복사한 보고서를 붙여 넣고 검토한 뒤 직접 제출하세요. GitHub 계정이 필요합니다.',
    'GitHub issues are public. Paste the copied report, review it and submit it yourself. A GitHub account is required.',
    'GitHub问题是公开的。请粘贴已复制的报告，检查后自行提交。需要GitHub账户。',
    'GitHub問題是公開的。請貼上已複製的報告，檢查後自行提交。需要GitHub帳號。',
    'GitHubのIssueは公開されます。コピーしたレポートを貼り付け、確認してから自分で送信してください。GitHubアカウントが必要です。',
  ],
  copied: [
    '보고서를 복사했습니다.',
    'Report copied.',
    '已复制报告。',
    '已複製報告。',
    'レポートをコピーしました。',
  ],
  saved: [
    '보고서를 저장했습니다.',
    'Report saved.',
    '已保存报告。',
    '已儲存報告。',
    'レポートを保存しました。',
  ],
  failed: [
    '작업을 완료하지 못했습니다. 다시 시도하세요.',
    'Could not complete this action. Try again.',
    '无法完成此操作，请重试。',
    '無法完成此操作，請重試。',
    '操作を完了できませんでした。もう一度お試しください。',
  ],
  native: [
    '진단 수집은 데스크톱 앱에서 사용할 수 있습니다.',
    'Diagnostic capture is available in the desktop app.',
    '诊断收集可在桌面应用中使用。',
    '診斷收集可在桌面應用程式中使用。',
    '診断の取得はデスクトップアプリで利用できます。',
  ],
  busy: ['처리 중…', 'Working…', '正在处理…', '處理中…', '処理中…'],
} as const;

export function diagnosticText(language: Language, key: keyof typeof messages) {
  return messages[key][['ko', 'en', 'zh-CN', 'zh-TW', 'ja'].indexOf(language)] ?? messages[key][0];
}
