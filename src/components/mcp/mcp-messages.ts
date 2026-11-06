// Text for the external sources (MCP) settings. Order: ko, en, zh-CN, zh-TW, ja.
import type { Language } from '../../lib/types';

const messages = {
  title: [
    '외부 자료 (MCP)',
    'External sources (MCP)',
    '外部资料 (MCP)',
    '外部資料 (MCP)',
    '外部資料 (MCP)',
  ],
  intro: [
    '내 컴퓨터에서 실행하는 MCP 서버의 검색 도구로 외부 자료를 찾아 답변에 함께 쓸 수 있습니다. 서버에는 질문 문장만 보내고, 내 문서 내용은 보내지 않습니다. 외부 질문 허용과 질문별 선택이 모두 켜져 있을 때만 사용합니다.',
    'Use the search tool of an MCP server running on this computer to add outside material to answers. Only the question is sent to the server, never your documents. It is used only when external questions are allowed and you choose it for a question.',
    '使用本机运行的 MCP 服务器搜索工具查找外部资料并用于回答。只向服务器发送问题，不发送你的文档内容。仅在允许外部提问且你为该问题选择时使用。',
    '使用本機執行的 MCP 伺服器搜尋工具尋找外部資料並用於回答。只向伺服器傳送問題，不傳送你的文件內容。僅在允許外部提問且你為該問題選擇時使用。',
    'このコンピューターで動く MCP サーバーの検索ツールで外部資料を探し、回答に使えます。サーバーには質問文だけを送り、文書の内容は送りません。外部への質問を許可し、質問ごとに選んだときだけ使います。',
  ],
  enabled: [
    '외부 자료 사용',
    'Use external sources',
    '使用外部资料',
    '使用外部資料',
    '外部資料を使う',
  ],
  add: ['서버 추가', 'Add server', '添加服务器', '新增伺服器', 'サーバーを追加'],
  remove: ['삭제', 'Remove', '删除', '刪除', '削除'],
  name: ['이름', 'Name', '名称', '名稱', '名前'],
  command: ['실행 파일', 'Program', '程序', '程式', 'プログラム'],
  commandHint: [
    '셸 없이 직접 실행합니다. 예: node, uvx, 또는 실행 파일 전체 경로',
    'Started directly, without a shell. For example: node, uvx, or a full program path',
    '不经过 shell 直接启动。例如：node、uvx 或程序完整路径',
    '不經過 shell 直接啟動。例如：node、uvx 或程式完整路徑',
    'シェルを使わず直接起動します。例: node、uvx、またはプログラムのフルパス',
  ],
  args: [
    '인수 (한 줄에 하나)',
    'Arguments (one per line)',
    '参数（每行一个）',
    '參數（每行一個）',
    '引数（1 行に 1 つ）',
  ],
  probe: ['도구 확인', 'Check tools', '查看工具', '查看工具', 'ツールを確認'],
  probing: ['확인 중…', 'Checking…', '正在查看…', '正在查看…', '確認中…'],
  saveFirst: [
    '설정을 저장한 뒤에 도구를 확인할 수 있습니다.',
    'Save settings first, then check the tools.',
    '请先保存设置，再查看工具。',
    '請先儲存設定，再查看工具。',
    '設定を保存してからツールを確認できます。',
  ],
  searchTool: [
    '질문에 쓸 검색 도구',
    'Search tool for questions',
    '用于提问的搜索工具',
    '用於提問的搜尋工具',
    '質問に使う検索ツール',
  ],
  none: ['사용 안 함', 'Not used', '不使用', '不使用', '使わない'],
  argument: [
    '질문을 넣을 입력',
    'Input that receives the question',
    '接收问题的输入项',
    '接收問題的輸入項',
    '質問を入れる入力',
  ],
  readOnly: ['읽기 전용', 'Read-only', '只读', '唯讀', '読み取り専用'],
  notReadOnly: [
    '변경할 수 있는 도구라 사용하지 않음',
    'Not used: this tool can change things',
    '不使用：该工具可能会修改内容',
    '不使用：該工具可能會修改內容',
    '変更できるツールのため使いません',
  ],
  noTools: [
    '읽기 전용 검색 도구가 없습니다.',
    'This server has no read-only tools.',
    '该服务器没有只读工具。',
    '該伺服器沒有唯讀工具。',
    'このサーバーには読み取り専用ツールがありません。',
  ],
  noText: [
    '이 도구에는 질문을 넣을 텍스트 입력이 없습니다.',
    'This tool has no text input for a question.',
    '该工具没有可放入问题的文本输入。',
    '該工具沒有可放入問題的文字輸入。',
    'このツールには質問を入れるテキスト入力がありません。',
  ],
  maxServers: [
    '서버는 최대 8개까지 등록할 수 있습니다.',
    'Up to 8 servers can be registered.',
    '最多可注册 8 个服务器。',
    '最多可註冊 8 個伺服器。',
    'サーバーは最大 8 個まで登録できます。',
  ],
  start: [
    '프로그램을 시작하지 못했습니다. 실행 파일 이름이나 경로를 확인하세요.',
    'The program could not start. Check the program name or path.',
    '无法启动程序。请检查程序名称或路径。',
    '無法啟動程式。請檢查程式名稱或路徑。',
    'プログラムを起動できませんでした。名前またはパスを確認してください。',
  ],
  timedOut: [
    '서버가 15초 안에 응답하지 않았습니다.',
    'The server did not answer within 15 seconds.',
    '服务器在 15 秒内没有响应。',
    '伺服器在 15 秒內沒有回應。',
    'サーバーが 15 秒以内に応答しませんでした。',
  ],
  closed: [
    '서버가 응답 전에 종료되었습니다.',
    'The server stopped before answering.',
    '服务器在响应前已退出。',
    '伺服器在回應前已結束。',
    'サーバーが応答前に終了しました。',
  ],
  protocol: [
    '서버가 MCP 형식이 아닌 응답을 보냈습니다.',
    'The server replied with something that is not MCP.',
    '服务器返回了非 MCP 格式的内容。',
    '伺服器回傳了非 MCP 格式的內容。',
    'サーバーが MCP ではない応答を返しました。',
  ],
  server: [
    '서버가 오류를 보고했습니다.',
    'The server reported an error.',
    '服务器报告了错误。',
    '伺服器回報了錯誤。',
    'サーバーがエラーを報告しました。',
  ],
  busy: [
    '다른 서버를 확인하는 중입니다. 끝난 뒤 다시 시도하세요.',
    'Another server is being checked. Try again when it finishes.',
    '正在查看另一个服务器，完成后请重试。',
    '正在查看另一個伺服器，完成後請重試。',
    '別のサーバーを確認中です。終わってから再試行してください。',
  ],
  notFound: [
    '저장된 설정에 이 서버가 없습니다. 먼저 저장하세요.',
    'This server is not in the saved settings. Save first.',
    '已保存的设置中没有该服务器。请先保存。',
    '已儲存的設定中沒有該伺服器。請先儲存。',
    '保存済みの設定にこのサーバーがありません。先に保存してください。',
  ],
  unavailable: [
    '데스크톱 앱에서만 사용할 수 있습니다.',
    'Available only in the desktop app.',
    '仅在桌面应用中可用。',
    '僅在桌面應用程式中可用。',
    'デスクトップアプリでのみ使えます。',
  ],
} satisfies Record<string, [string, string, string, string, string]>;

const languages: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export type McpMessage = keyof typeof messages;

export function mcpText(language: Language, key: McpMessage) {
  return messages[key][languages[language]] ?? messages[key][1];
}
