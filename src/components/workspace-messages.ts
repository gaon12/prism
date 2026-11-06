import type { Language } from '../lib/types';
const messages = {
  aiTool: [
    'AI·모델 설정 열기',
    'Open AI and model settings',
    '打开AI与模型设置',
    '開啟AI與模型設定',
    'AI・モデルの設定を開く',
  ],
  recentTool: [
    '최근 검색 열기',
    'Open recent searches',
    '打开最近搜索',
    '開啟最近搜尋',
    '最近の検索を開く',
  ],
  tools: ['작업 도구', 'Workspace tools', '工作区工具', '工作區工具', 'ワークスペースのツール'],
  libraryToggle: [
    '라이브러리 패널 표시/숨기기',
    'Show or hide library panel',
    '显示或隐藏资料库面板',
    '顯示或隱藏資料庫面板',
    'ライブラリーパネルを表示・非表示',
  ],
  tasks: ['백그라운드 작업', 'Background tasks', '后台任务', '背景工作', 'バックグラウンドタスク'],
  indexStatus: ['색인 상태', 'Indexing status', '索引状态', '索引狀態', 'インデックスの状態'],
  collecting: [
    '파일 목록 수집 중',
    'Collecting filenames',
    '正在收集文件名',
    '正在收集檔名',
    'ファイル名を収集中',
  ],
  processing: [
    '문서 처리 중',
    'Processing documents',
    '正在处理文档',
    '正在處理文件',
    '文書を処理中',
  ],
  updating: [
    '파일 변경 반영 중',
    'Updating filenames',
    '正在更新文件名',
    '正在更新檔名',
    'ファイル名を更新中',
  ],
  waiting: [
    '전체 파일 수를 확인하고 있습니다.',
    'The final file count is not known yet.',
    '尚未确定文件总数。',
    '尚未確定檔案總數。',
    'ファイルの総数はまだ確定していません。',
  ],
  ready: ['작업 대기', 'Idle', '空闲', '閒置', '待機中'],
  statusError: [
    '색인 상태를 확인하지 못했습니다.',
    'Could not read indexing status.',
    '无法读取索引状态。',
    '無法讀取索引狀態。',
    'インデックスの状態を取得できませんでした。',
  ],
  filenames: ['파일명', 'Filenames', '文件名', '檔名', 'ファイル名'],
  documents: ['문서 본문', 'Document content', '文档正文', '文件內文', '文書の本文'],
} satisfies Record<string, readonly [string, string, string, string, string]>;
const index: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export const workspaceText = (language: Language, key: keyof typeof messages) =>
  messages[key][index[language]];
