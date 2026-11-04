// Texts of the first-run tour. Order in every array: ko, en, zh-CN, zh-TW, ja
// (the same order as src/lib/i18n.ts). `{x}` placeholders are filled by `onboardingText`.
import type { Language } from '../../lib/types';

const messages = {
  skip: ['나중에 하기', 'Skip for now', '稍后再说', '稍後再說', 'あとで'],
  back: ['이전', 'Back', '上一步', '上一步', '戻る'],
  next: ['다음', 'Next', '下一步', '下一步', '次へ'],
  finish: [
    'Prism 시작하기',
    'Start using Prism',
    '开始使用 Prism',
    '開始使用 Prism',
    'Prism を使い始める',
  ],
  saving: ['저장 중…', 'Saving…', '正在保存…', '正在儲存…', '保存中…'],
  stepOf: [
    '{n}/{total}단계',
    'Step {n} of {total}',
    '第 {n} 步，共 {total} 步',
    '第 {n} 步，共 {total} 步',
    'ステップ {n}/{total}',
  ],

  // Step names in the side rail
  welcome: ['시작', 'Welcome', '欢迎', '歡迎', 'ようこそ'],
  shortcut: ['단축키', 'Shortcut', '快捷键', '快捷鍵', 'ショートカット'],
  folders: ['검색 폴더', 'Folders', '搜索文件夹', '搜尋資料夾', 'フォルダー'],
  privacy: ['개인정보', 'Privacy', '隐私', '隱私', 'プライバシー'],
  ready: ['완료', 'Ready', '完成', '完成', '完了'],

  // Welcome
  welcomeTitle: [
    'Prism에 오신 것을 환영해요',
    'Welcome to Prism',
    '欢迎使用 Prism',
    '歡迎使用 Prism',
    'Prism へようこそ',
  ],
  welcomeBody: [
    '파일 이름, 문서 내용, 클립보드 기록을 한 검색창에서 찾아요. 몇 가지만 정하면 바로 쓸 수 있어요. 모든 항목은 나중에 설정에서 바꿀 수 있어요.',
    'Search file names, document text and clipboard history from one box. A few quick choices and you are set; everything can be changed later in Settings.',
    '在一个搜索框中查找文件名、文档内容和剪贴板记录。只需几个简单选择即可开始，之后都可以在设置中更改。',
    '在一個搜尋框中尋找檔名、文件內容和剪貼簿記錄。只需幾個簡單選擇即可開始，之後都能在設定中變更。',
    'ファイル名、文書の内容、クリップボード履歴をひとつの検索欄で探せます。いくつか選ぶだけで始められ、あとから設定で変更できます。',
  ],
  language: ['언어', 'Language', '语言', '語言', '言語'],
  theme: ['테마', 'Theme', '主题', '主題', 'テーマ'],
  themeSystem: ['시스템 설정', 'Match system', '跟随系统', '跟隨系統', 'システムに合わせる'],
  themeLight: ['밝게', 'Light', '浅色', '淺色', 'ライト'],
  themeDark: ['어둡게', 'Dark', '深色', '深色', 'ダーク'],

  // Shortcut
  shortcutTitle: [
    '어디서든 단축키로 여세요',
    'Open Prism from anywhere',
    '随时随地用快捷键打开',
    '隨時隨地用快捷鍵開啟',
    'どこからでもショートカットで開く',
  ],
  shortcutBody: [
    '다른 프로그램을 쓰는 중에도 이 키를 누르면 작은 검색창이 바로 떠요.',
    'Press these keys in any app and a small search window appears.',
    '在任何程序中按下这些键，都会弹出小搜索窗口。',
    '在任何程式中按下這些鍵，都會跳出小搜尋視窗。',
    'どのアプリを使っていても、このキーで小さな検索ウィンドウが開きます。',
  ],
  change: ['다른 키로 바꾸기', 'Change keys', '更换按键', '更換按鍵', 'キーを変更'],
  recording: [
    '새 단축키를 누르세요. 취소하려면 Esc',
    'Press the new keys. Esc to cancel',
    '请按下新的快捷键，按 Esc 取消',
    '請按下新的快捷鍵，按 Esc 取消',
    '新しいキーを押してください。Esc で取り消し',
  ],
  needsModifier: [
    'Ctrl, Alt 또는 Win 키와 함께 눌러 주세요.',
    'Hold Ctrl, Alt or Win together with the key.',
    '请同时按住 Ctrl、Alt 或 Win 键。',
    '請同時按住 Ctrl、Alt 或 Win 鍵。',
    'Ctrl、Alt、Win のいずれかと一緒に押してください。',
  ],
  unsupported: [
    '이 키는 단축키로 쓸 수 없어요. 글자, 숫자, F1–F24, Space 중에서 골라 주세요.',
    'That key cannot be used. Pick a letter, digit, F1–F24 or Space.',
    '此键无法使用，请选择字母、数字、F1–F24 或空格。',
    '此鍵無法使用，請選擇字母、數字、F1–F24 或空白鍵。',
    'このキーは使えません。文字、数字、F1–F24、Space から選んでください。',
  ],

  // Folders
  foldersTitle: [
    '어디를 검색할까요?',
    'Where should Prism search?',
    '要搜索哪些位置？',
    '要搜尋哪些位置？',
    'どこを検索しますか？',
  ],
  foldersBody: [
    '선택한 폴더의 파일 이름을 검색해요. "문서 내용"을 켜면 문서 안의 글도 찾아요. 색인은 이 PC에만 저장돼요.',
    'Prism searches file names in the folders you choose. Turn on "Document text" to search inside documents too. The index stays on this PC.',
    'Prism 会搜索所选文件夹中的文件名。开启"文档内容"后也会搜索文档内文字。索引只保存在本机。',
    'Prism 會搜尋所選資料夾中的檔名。開啟「文件內容」後也會搜尋文件內文字。索引只儲存在本機。',
    '選んだフォルダーのファイル名を検索します。「文書の内容」をオンにすると文書内の文字も検索します。インデックスはこの PC にだけ保存されます。',
  ],
  documentText: ['문서 내용', 'Document text', '文档内容', '文件內容', '文書の内容'],
  addFolder: ['폴더 추가', 'Add folder', '添加文件夹', '新增資料夾', 'フォルダーを追加'],
  loadingFolders: [
    '추천 폴더를 찾는 중…',
    'Finding suggested folders…',
    '正在查找推荐文件夹…',
    '正在尋找推薦資料夾…',
    'おすすめのフォルダーを探しています…',
  ],
  noFolders: [
    '아직 고른 폴더가 없어요. 폴더를 추가하거나 나중에 설정에서 추가할 수 있어요.',
    'No folders yet. Add one now, or later in Settings.',
    '还没有文件夹。现在添加，或稍后在设置中添加。',
    '還沒有資料夾。現在新增，或稍後在設定中新增。',
    'まだフォルダーがありません。今追加するか、あとで設定から追加できます。',
  ],

  // Privacy
  privacyTitle: ['기록과 AI 사용 방식', 'History and AI', '记录与 AI', '記錄與 AI', '履歴と AI'],
  clipboardHistory: [
    '클립보드 기록 보관',
    'Keep clipboard history',
    '保存剪贴板记录',
    '保存剪貼簿記錄',
    'クリップボード履歴を保存',
  ],
  clipboardHistoryHint: [
    '복사한 글과 링크를 이 PC에 저장해 다시 찾을 수 있어요. 비밀번호 관리자가 제외를 요청한 항목은 저장하지 않아요.',
    'Copied text and links are saved on this PC so you can find them again. Items a password manager marks as private are never saved.',
    '复制的文字和链接保存在本机，方便再次查找。密码管理器标记为私密的内容不会保存。',
    '複製的文字和連結儲存在本機，方便再次尋找。密碼管理器標記為私密的內容不會儲存。',
    'コピーした文字やリンクをこの PC に保存して、あとで探せます。パスワード管理アプリが除外を求めた項目は保存しません。',
  ],
  clipboardImages: [
    '복사한 이미지도 보관',
    'Include copied images',
    '也保存复制的图片',
    '也保存複製的圖片',
    'コピーした画像も保存',
  ],
  startOnLogin: [
    '로그인 시 실행',
    'Start at login',
    '登录时启动',
    '登入時啟動',
    'ログイン時に開始',
  ],
  portableStartup: [
    '포터블에서는 로그인 자동 실행을 사용하지 않아요.',
    'Login startup is disabled in portable mode.',
    '便携模式不支持登录时启动。',
    '可攜模式不支援登入時啟動。',
    'ポータブル版ではログイン時の自動起動は無効です。',
  ],
  startOnLoginHint: [
    '단축키가 언제나 바로 동작하도록 백그라운드에서 조용히 대기해요.',
    'Waits quietly in the background so the shortcut always works.',
    '在后台安静等待，确保快捷键随时可用。',
    '在背景安靜等待，確保快捷鍵隨時可用。',
    'ショートカットがいつでも使えるよう、バックグラウンドで待機します。',
  ],
  externalQuestions: [
    '외부 AI 서비스에 질문 보내기 허용',
    'Allow questions to an external AI service',
    '允许向外部 AI 服务发送问题',
    '允許向外部 AI 服務傳送問題',
    '外部の AI サービスへの質問を許可',
  ],
  externalQuestionsHint: [
    '꺼 두면 질문과 문서 내용을 외부 AI로 보내지 않아요. 외부 AI를 쓰려면 질문 허용과 폴더별 내용 허용을 따로 선택해야 해요.',
    'While off, questions and document text are not sent to external AI. External use requires separate question and per-folder content permissions.',
    '关闭时，不会向外部 AI 发送问题或文档正文。使用外部 AI 需要分别允许问题和各文件夹正文的发送。',
    '關閉時，不會向外部 AI 傳送問題或文件內文。使用外部 AI 需要分別允許問題及各資料夾內文的傳送。',
    'オフの間、質問や文書本文を外部 AI に送りません。外部 AI の利用には、質問と各フォルダー本文の送信を個別に許可する必要があります。',
  ],
  folderExternal: [
    '외부 AI에 이 폴더 내용 보내기 허용',
    'Allow sending this folder’s text to external AI',
    '允许将此文件夹内容发送给外部 AI',
    '允許將此資料夾內容傳送給外部 AI',
    'このフォルダーの内容を外部 AI に送ることを許可',
  ],
  modelsNote: [
    '검색용 EmbeddingGemma 2와 답변용 AI는 서로 달라요. 설정 › AI에서 검색 모델을 설치하고, 답변용으로는 직접 실행한 로컬 서버나 외부 API를 연결해요.',
    'EmbeddingGemma 2 for search and AI for writing replies are separate. In Settings › AI, install the search model and connect a user-run local server or external API for replies.',
    '用于搜索的 EmbeddingGemma 2 与生成回复的 AI 不同。在 设置 › AI 中安装搜索模型，并连接自行运行的本地服务器或外部 API 来生成回复。',
    '用於搜尋的 EmbeddingGemma 2 與產生回覆的 AI 不同。在 設定 › AI 中安裝搜尋模型，並連接自行執行的本機伺服器或外部 API 來產生回覆。',
    '検索用の EmbeddingGemma 2 と回答用 AI は別です。設定 › AI で検索モデルを導入し、回答には自分で起動したローカルサーバーか外部 API を接続します。',
  ],

  // Ready
  readyTitle: ['준비됐어요', 'You are all set', '一切就绪', '一切就緒', '準備ができました'],
  readyBody: [
    '이렇게 써 보세요.',
    'A few things to try:',
    '可以试试这些：',
    '可以試試這些：',
    '試してみましょう：',
  ],
  tipShortcut: [
    '{keys}를 눌러 어디서든 빠른 검색창 열기',
    'Press {keys} anywhere to open quick search',
    '在任意位置按 {keys} 打开快速搜索',
    '在任意位置按 {keys} 開啟快速搜尋',
    'どこでも {keys} を押してクイック検索を開く',
  ],
  tipFile: [
    '파일 이름 일부만 입력해도 찾아요. 예: 견적서',
    'Type part of a file name, e.g. invoice',
    '输入部分文件名即可，例如：报价单',
    '輸入部分檔名即可，例如：報價單',
    'ファイル名の一部で検索、例：見積書',
  ],
  tipCalc: [
    '검색창에 바로 계산하기. 예: 1280*0.15',
    'Calculate right in the search box, e.g. 1280*0.15',
    '直接在搜索框里计算，例如：1280*0.15',
    '直接在搜尋框中計算，例如：1280*0.15',
    '検索欄でそのまま計算、例：1280*0.15',
  ],
  tipClipboard: [
    '클립보드 탭에서 전에 복사한 내용 다시 붙여넣기',
    'Paste something you copied earlier from the Clipboard tab',
    '在剪贴板标签页中再次粘贴之前复制的内容',
    '在剪貼簿分頁中再次貼上先前複製的內容',
    'クリップボードタブから以前コピーした内容を貼り付け',
  ],
} satisfies Record<string, readonly [string, string, string, string, string]>;

export type OnboardingKey = keyof typeof messages;
const order: Language[] = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'];

/** Tour text in `language`, with `{name}` placeholders replaced from `values`. */
export function onboardingText(
  language: Language,
  key: OnboardingKey,
  values: Record<string, string | number> = {},
) {
  const text = messages[key][order.indexOf(language)] ?? messages[key][1];
  return Object.entries(values).reduce(
    (result, [name, value]) => result.replaceAll(`{${name}}`, String(value)),
    text,
  );
}
