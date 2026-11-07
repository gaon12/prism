import type { Language } from '../../lib/types';

const messages = {
  title: [
    'Office 원본 페이지',
    'Original Office pages',
    'Office 原始页面',
    'Office 原始頁面',
    'Officeの元のページ',
  ],
  description: [
    '컬렉션의 DOCX·PPTX·XLSX를 원래 페이지 배치로 읽습니다. 엔진이 없으면 텍스트 미리보기를 사용합니다.',
    'Read collection DOCX, PPTX and XLSX files with their page layout. Without the engine, a text preview is used.',
    '按原始页面布局阅读收藏集中的 DOCX、PPTX 和 XLSX。没有引擎时使用文本预览。',
    '按原始頁面配置閱讀收藏集中的 DOCX、PPTX 和 XLSX。沒有引擎時使用文字預覽。',
    'コレクションのDOCX・PPTX・XLSXを元のページ配置で読みます。エンジンがない場合はテキストでプレビューします。',
  ],
  absent: [
    '엔진 없음',
    'Engine not installed',
    '未安装引擎',
    '未安裝引擎',
    'エンジン未インストール',
  ],
  present: [
    '엔진 파일 있음',
    'Engine files present',
    '引擎文件已存在',
    '引擎檔案已存在',
    'エンジンファイルあり',
  ],
  verify: [
    '페이지를 열 때마다 엔진 파일을 검증합니다. 기존 텍스트 미리보기는 원본을 다시 가져오면 페이지로 바뀝니다.',
    'Engine files are verified each time pages are opened. Reimport an original to replace an existing text preview with pages.',
    '每次打开页面时都会验证引擎文件。重新导入原始文件可将现有文本预览替换为页面。',
    '每次開啟頁面時都會驗證引擎檔案。重新匯入原始檔案可將現有文字預覽替換為頁面。',
    'ページを開くたびにエンジンファイルを検証します。既存のテキストプレビューは元のファイルを再取り込みするとページに変わります。',
  ],
  unpublished: [
    '다운로드용 패키지는 아직 공개되지 않았습니다. Prism용으로 검증된 ZIP 패키지가 있으면 선택할 수 있습니다.',
    'The download package is not published yet. You can select a ZIP package verified for Prism.',
    '下载包尚未发布。可以选择经过 Prism 验证的 ZIP 包。',
    '下載套件尚未發布。可以選擇經過 Prism 驗證的 ZIP 套件。',
    'ダウンロード用パッケージは未公開です。Prism用に検証されたZIPがある場合は選択できます。',
  ],
  archive: ['ZIP 용량', 'ZIP size', 'ZIP 大小', 'ZIP 大小', 'ZIPサイズ'],
  disk: ['설치 후 용량', 'Installed size', '安装后大小', '安裝後大小', 'インストール後のサイズ'],
  choose: [
    '엔진 ZIP 선택',
    'Choose engine ZIP',
    '选择引擎 ZIP',
    '選擇引擎 ZIP',
    'エンジンZIPを選択',
  ],
  stop: ['설치 중단', 'Cancel installation', '取消安装', '取消安裝', 'インストールを中止'],
  stopping: ['중단 중…', 'Cancelling…', '正在取消…', '正在取消…', '中止しています…'],
  installing: [
    '엔진 설치·검증 중',
    'Installing and verifying engine',
    '正在安装和验证引擎',
    '正在安裝與驗證引擎',
    'エンジンをインストール・検証中',
  ],
  waiting: [
    '진행 중인 페이지 작업을 기다리는 중',
    'Waiting for the current page operation',
    '正在等待当前页面操作',
    '正在等候目前頁面作業',
    '実行中のページ処理を待っています',
  ],
  finalizing: [
    '설치를 마무리하는 중',
    'Finishing installation',
    '正在完成安装',
    '正在完成安裝',
    'インストールを完了しています',
  ],
  files: [
    '{n} / {total}개 파일',
    '{n} of {total} files',
    '{n} / {total} 个文件',
    '{n} / {total} 個檔案',
    '{total}ファイル中{n}件',
  ],
  cancelled: [
    '설치를 중단했습니다.',
    'Installation cancelled.',
    '已取消安装。',
    '已取消安裝。',
    'インストールを中止しました。',
  ],
  complete: [
    '엔진 설치를 마쳤습니다.',
    'Engine installation completed.',
    '引擎安装完成。',
    '引擎安裝完成。',
    'エンジンのインストールが完了しました。',
  ],
  unsupported: [
    '현재 원본 페이지 엔진은 Windows x64·Linux x64에서 지원합니다.',
    'The page engine currently supports Windows x64 and Linux x64.',
    '原始页面引擎目前支持 Windows x64 和 Linux x64。',
    '原始頁面引擎目前支援 Windows x64 與 Linux x64。',
    '元のページ用エンジンは現在Windows x64・Linux x64に対応しています。',
  ],
  desktop: [
    '엔진은 데스크톱 앱에서 준비할 수 있습니다.',
    'Prepare the engine in the desktop app.',
    '请在桌面应用中准备引擎。',
    '請在桌面應用程式中準備引擎。',
    'エンジンはデスクトップアプリで準備できます。',
  ],
  failed: [
    '엔진 상태를 불러오지 못했습니다.',
    'Could not load engine status.',
    '无法加载引擎状态。',
    '無法載入引擎狀態。',
    'エンジンの状態を取得できませんでした。',
  ],
  retry: ['다시 확인', 'Check again', '重新检查', '重新檢查', '再確認'],
  loading: [
    '엔진 상태 확인 중…',
    'Checking engine status…',
    '正在检查引擎状态…',
    '正在檢查引擎狀態…',
    'エンジンの状態を確認中…',
  ],
} as const;
const indexes: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export function officeText(
  language: Language,
  key: keyof typeof messages,
  values?: Record<string, string | number>,
) {
  let text: string = messages[key][indexes[language]];
  for (const [name, value] of Object.entries(values ?? {}))
    text = text.replaceAll(`{${name}}`, String(value));
  return text;
}
