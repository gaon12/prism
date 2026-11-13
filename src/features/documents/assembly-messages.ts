import type { Language } from '../../lib/types';

const messages = {
  enable: [
    '항목·표로 구성',
    'Compose with fields and tables',
    '使用字段和表格组织',
    '使用欄位與表格組織',
    '項目・表で構成',
  ],
  hint: [
    '항목과 표를 먼저 검토합니다. AI는 선택한 자료에서 값을 찾고, 검토한 구성에 맞춰 본문을 만듭니다. 누락 안내는 선택한 발췌문에만 해당합니다.',
    'Review the fields and tables first. AI finds values in the selected sources and fills the reviewed layout. Missing-value notices refer only to the supplied excerpts.',
    '请先审阅字段和表格。AI从所选资料中提取值并按审阅的结构生成正文。缺失提示仅针对提供的摘录。',
    '請先審閱欄位與表格。AI從所選資料中擷取值並依審閱的結構產生內文。缺漏提示僅針對提供的摘錄。',
    '項目と表を先に確認します。AIが選択した資料から値を探し、確認済みの構成に沿って本文を作成します。未確認の案内は提供された抜粋のみが対象です。',
  ],
  language: [
    '누락 안내 언어',
    'Missing-value notice language',
    '缺失提示语言',
    '缺漏提示語言',
    '未確認の案内の言語',
  ],
  addField: ['항목 추가', 'Add field', '添加字段', '新增欄位', '項目を追加'],
  addTable: [
    '두 열 표 추가',
    'Add two-column table',
    '添加两列表格',
    '新增兩欄表格',
    '2列の表を追加',
  ],
  addRow: ['행 추가', 'Add row', '添加行', '新增列', '行を追加'],
  remove: ['삭제', 'Remove', '删除', '刪除', '削除'],
  field: ['항목', 'Field', '字段', '欄位', '項目'],
  value: ['값', 'Value', '值', '值', '値'],
  table: ['표', 'Table', '表格', '表格', '表'],
  label: ['항목 이름', 'Field name', '字段名称', '欄位名稱', '項目名'],
  instruction: [
    '자료에서 찾을 내용',
    'What to find in sources',
    '要在资料中查找的内容',
    '要在資料中尋找的內容',
    '資料から探す内容',
  ],
  notice: [
    '값을 찾지 못하면 안내 표시',
    'Show a notice if no value is found',
    '未找到值时显示提示',
    '未找到值時顯示提示',
    '値が見つからない場合に案内を表示',
  ],
  firstColumn: [
    '첫 번째 열 제목',
    'First column heading',
    '第一列标题',
    '第一欄標題',
    '1列目の見出し',
  ],
  secondColumn: [
    '두 번째 열 제목',
    'Second column heading',
    '第二列标题',
    '第二欄標題',
    '2列目の見出し',
  ],
  empty: [
    '이 섹션에 항목이나 표를 추가하세요.',
    'Add a field or table to this section.',
    '请向此章节添加字段或表格。',
    '請在此章節新增欄位或表格。',
    'この項目にフィールドまたは表を追加してください。',
  ],
  invalid: [
    '모든 섹션에 항목을 넣고, 빈 이름과 표 제목을 확인하세요. 최대 64개 항목을 사용할 수 있습니다.',
    'Add fields to every section and check empty names and table headings. Up to 64 fields are supported.',
    '请为每个章节添加字段，并检查空名称和表格标题。最多支持64个字段。',
    '請為每個章節新增欄位，並檢查空白名稱與表格標題。最多支援64個欄位。',
    '各項目にフィールドを追加し、空の名前や表の見出しを確認してください。最大64フィールドに対応します。',
  ],
  up: ['위로 이동', 'Move up', '上移', '上移', '上へ移動'],
  down: ['아래로 이동', 'Move down', '下移', '下移', '下へ移動'],
} satisfies Record<string, readonly string[]>;
const index: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export const assemblyText = (language: Language, key: keyof typeof messages) =>
  messages[key][index[language]];
