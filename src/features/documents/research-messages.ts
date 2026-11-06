import type { Language } from '../../lib/types';
import type { ExternalEvidence } from '../../lib/rag';

const messages = {
  optIn: [
    '이번 생성에서 외부 자료도 찾기',
    'Also research external sources for this operation',
    '本次生成也搜索外部资料',
    '本次生成也搜尋外部資料',
    '今回の生成で外部資料も検索',
  ],
  query: ['MCP 검색 문장', 'MCP research query', 'MCP搜索语句', 'MCP搜尋句子', 'MCP検索文'],
  help: [
    '입력한 검색 문장만 등록된 MCP 서버에 보냅니다. 생성 지시와 문서 본문은 검색 요청에 포함하지 않습니다. 매 생성마다 직접 선택하세요.',
    'Only this query goes to the configured MCP servers. Generation instructions and draft text stay outside the search request. Opt in separately for each operation.',
    '仅将此语句发送给已配置的MCP服务器。生成指示和草稿正文不包含在搜索请求中，每次生成需单独选择。',
    '僅將此句子傳送給已設定的MCP伺服器。生成指示和草稿內文不包含在搜尋請求中，每次生成需個別選擇。',
    'この検索文だけを設定済みMCPサーバーに送ります。生成指示や下書き本文は検索に含めません。生成ごとに選択してください。',
  ],
  configure: [
    '외부 검색을 사용하려면 AI 설정에서 읽기 전용 MCP 검색 도구를 연결하세요.',
    'Configure a read-only MCP search tool in AI settings to enable external research.',
    '请在AI设置中配置只读MCP搜索工具以启用外部搜索。',
    '請在AI設定中連接唯讀MCP搜尋工具以啟用外部搜尋。',
    'AI設定で読み取り専用のMCP検索ツールを接続してください。',
  ],
  invalid: [
    '검색 문장은 비어 있지 않은 한 줄, UTF-8 4,096바이트 이내여야 합니다.',
    'Use a nonempty single-line query of at most 4,096 UTF-8 bytes.',
    '搜索语句须为非空单行，且不超过4,096个UTF-8字节。',
    '搜尋句子須為非空單行，且不超過4,096個UTF-8位元組。',
    '検索文は空でない1行、UTF-8で4,096バイト以内にしてください。',
  ],
  heading: [
    '이 문서의 외부 자료',
    'External research for this document',
    '此文档的外部资料',
    '此文件的外部資料',
    'この文書の外部資料',
  ],
  untrusted: [
    '외부 검색 결과이며 사실 검증을 뜻하지 않습니다.',
    'External search results are not verified facts.',
    '外部搜索结果不代表事实已经核实。',
    '外部搜尋結果不代表事實已經核實。',
    '外部検索結果であり、事実確認済みではありません。',
  ],
  truncated: [
    '일부만 표시합니다.',
    'Only part of the result is shown.',
    '仅显示部分结果。',
    '僅顯示部分結果。',
    '結果の一部を表示しています。',
  ],
  unavailable: [
    '외부 자료의 저장 기록을 불러오지 못했습니다.',
    'Saved external research could not be loaded.',
    '无法加载已保存的外部资料。',
    '無法載入已儲存的外部資料。',
    '保存済みの外部資料を読み込めませんでした。',
  ],
  fingerprint: [
    '서버 설정 지문',
    'Server configuration fingerprint',
    '服务器配置指纹',
    '伺服器設定指紋',
    'サーバー設定のフィンガープリント',
  ],
} as const;
const outcomes: Record<ExternalEvidence['outcome'], readonly string[]> = {
  ok: ['검색됨', 'Found', '已找到', '已找到', '検索済み'],
  empty: ['결과 없음', 'No results', '无结果', '無結果', '結果なし'],
  timedOut: ['시간 초과', 'Timed out', '超时', '逾時', 'タイムアウト'],
  cancelled: ['취소됨', 'Cancelled', '已取消', '已取消', 'キャンセル済み'],
  schema: [
    '검색 도구 입력 불일치',
    'Search tool input mismatch',
    '搜索工具输入不匹配',
    '搜尋工具輸入不符',
    '検索ツールの入力が不一致',
  ],
  start: [
    '서버 시작 실패',
    'Server could not start',
    '服务器无法启动',
    '伺服器無法啟動',
    'サーバー起動失敗',
  ],
  failed: ['검색 실패', 'Search failed', '搜索失败', '搜尋失敗', '検索失敗'],
  changed: ['설정 변경됨', 'Settings changed', '设置已更改', '設定已變更', '設定が変更されました'],
  skipped: [
    '검색 예산 초과',
    'Research budget exhausted',
    '搜索预算已用尽',
    '搜尋預算已用盡',
    '検索予算を超過',
  ],
};
const index = (language: Language) => ({ ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 })[language];
export const researchText = (language: Language, key: keyof typeof messages) =>
  messages[key][index(language)];
export const researchOutcome = (language: Language, outcome: ExternalEvidence['outcome']) =>
  outcomes[outcome][index(language)];
