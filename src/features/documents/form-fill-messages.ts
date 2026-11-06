// Text for the form-filling panel. Order: ko, en, zh-CN, zh-TW, ja.
import type { Language } from '../../lib/types';

const messages = {
  title: [
    '기존 양식 채우기',
    'Fill an existing form',
    '填写现有表单',
    '填寫現有表單',
    '既存の書式に記入',
  ],
  intro: [
    'DOCX나 HWPX 양식의 {{이름}} 자리표시자와 ___ 빈칸을 찾아 채웁니다. 서식은 그대로 두고, 원본은 바꾸지 않고 새 파일로 저장합니다.',
    'Finds {{name}} placeholders and ___ blanks in a DOCX or HWPX form and fills them. Formatting stays as it is, and the result is saved as a new file; the original is not changed.',
    '查找 DOCX 或 HWPX 表单中的 {{名称}} 占位符和 ___ 空白并填写。保留格式，结果另存为新文件，不修改原件。',
    '尋找 DOCX 或 HWPX 表單中的 {{名稱}} 預留位置和 ___ 空白並填寫。保留格式，結果另存為新檔案，不修改原件。',
    'DOCX・HWPX 書式の {{名前}} プレースホルダーと ___ 空欄を見つけて記入します。書式はそのまま、原本は変えずに新しいファイルとして保存します。',
  ],
  choose: ['양식 파일 열기', 'Open a form', '打开表单文件', '開啟表單檔案', '書式ファイルを開く'],
  chooseOther: ['다른 양식', 'Another form', '其他表单', '其他表單', '別の書式'],
  noFields: [
    '채울 곳을 찾지 못했습니다. 양식에 {{이름}} 또는 ___(밑줄 3개 이상)을 넣어 두세요.',
    'No fields found. Put {{name}} or ___ (three or more underscores) where values go.',
    '未找到可填写的位置。请在表单中放入 {{名称}} 或 ___（三个以上下划线）。',
    '找不到可填寫的位置。請在表單中放入 {{名稱}} 或 ___（三個以上底線）。',
    '記入箇所が見つかりません。書式に {{名前}} または ___（3 つ以上の下線）を入れてください。',
  ],
  blank: ['빈칸 {n}', 'Blank {n}', '空白 {n}', '空白 {n}', '空欄 {n}'],
  fieldCount: [
    '채울 곳 {count}개',
    '{count} fields',
    '{count} 个位置',
    '{count} 個位置',
    '{count} 箇所',
  ],
  save: [
    '채워서 새 파일로 저장',
    'Fill and save as new file',
    '填写并另存为新文件',
    '填寫並另存為新檔案',
    '記入して新しいファイルに保存',
  ],
  saving: ['저장하는 중…', 'Saving…', '正在保存…', '正在儲存…', '保存中…'],
  saved: [
    '저장했습니다: {path}',
    'Saved: {path}',
    '已保存：{path}',
    '已儲存：{path}',
    '保存しました: {path}',
  ],
  emptyKept: [
    '비워 둔 칸은 양식 그대로 남습니다.',
    'Fields left empty stay as they are in the form.',
    '留空的位置保持表单原样。',
    '留空的位置保持表單原樣。',
    '空のままの欄は書式のまま残ります。',
  ],
  unsupported: [
    'DOCX 또는 HWPX 양식만 채울 수 있습니다.',
    'Only DOCX or HWPX forms can be filled.',
    '只能填写 DOCX 或 HWPX 表单。',
    '只能填寫 DOCX 或 HWPX 表單。',
    'DOCX または HWPX の書式のみ記入できます。',
  ],
  damaged: [
    '파일이 손상되었거나 읽을 수 없는 구조입니다.',
    'The file is damaged or has a structure that cannot be read.',
    '文件已损坏或结构无法读取。',
    '檔案已損毀或結構無法讀取。',
    'ファイルが破損しているか、読み取れない構造です。',
  ],
  tooLarge: [
    '양식 파일이 너무 큽니다.',
    'The form file is too large.',
    '表单文件过大。',
    '表單檔案過大。',
    '書式ファイルが大きすぎます。',
  ],
  io: [
    '파일을 읽거나 저장하지 못했습니다. 다른 위치를 고르거나 파일이 열려 있는지 확인하세요.',
    'Could not read or save the file. Choose another location or check whether the file is open elsewhere.',
    '无法读取或保存文件。请选择其他位置，或检查文件是否在别处打开。',
    '無法讀取或儲存檔案。請選擇其他位置，或確認檔案是否在別處開啟。',
    'ファイルを読み書きできませんでした。別の場所を選ぶか、ファイルが開かれていないか確認してください。',
  ],
  input: [
    '이 위치에는 저장할 수 없습니다. 원본과 다른 이름으로, 같은 형식(.docx/.hwpx)의 파일을 고르세요.',
    'Cannot save there. Choose a different name than the original, with the same type (.docx/.hwpx).',
    '无法保存到该位置。请选择与原件不同、类型相同（.docx/.hwpx）的文件名。',
    '無法儲存到該位置。請選擇與原件不同、類型相同（.docx/.hwpx）的檔名。',
    'そこには保存できません。原本と別の名前で、同じ形式（.docx/.hwpx）のファイルを選んでください。',
  ],
  invalid: [
    '입력한 값에 문서에 넣을 수 없는 제어 문자가 있습니다. 해당 문자를 지우고 다시 저장하세요.',
    'A value contains control characters that cannot go into a document. Remove them and save again.',
    '输入的值包含无法写入文档的控制字符。请删除后重新保存。',
    '輸入的值包含無法寫入文件的控制字元。請刪除後重新儲存。',
    '入力値に文書に入れられない制御文字があります。削除してもう一度保存してください。',
  ],
  changed: [
    '열어 둔 뒤에 양식 파일이 바뀌었습니다. 다시 열어 칸을 확인한 뒤 저장하세요. 입력한 값은 유지됩니다.',
    'The form file changed after it was opened. Reload it, check the fields, then save. Your values are kept.',
    '打开后表单文件已更改。请重新载入、核对位置后再保存。已输入的值会保留。',
    '開啟後表單檔案已變更。請重新載入、核對位置後再儲存。已輸入的值會保留。',
    '開いた後に書式ファイルが変更されました。再読み込みして欄を確認してから保存してください。入力値は保持されます。',
  ],
  reload: ['다시 열기', 'Reload', '重新载入', '重新載入', '再読み込み'],
  timedOut: [
    '양식 처리 시간이 너무 오래 걸려 중단했습니다.',
    'Processing the form took too long and was stopped.',
    '处理表单耗时过长，已停止。',
    '處理表單耗時過長，已停止。',
    '書式の処理に時間がかかりすぎたため中止しました。',
  ],
  busy: [
    '다른 양식 작업이 끝난 뒤에 다시 시도하세요.',
    'Try again after the other form operation finishes.',
    '请在其他表单操作完成后重试。',
    '請在其他表單作業完成後重試。',
    '別の書式処理が終わってから再試行してください。',
  ],
  unavailable: [
    '양식 채우기는 데스크톱 앱에서만 쓸 수 있습니다.',
    'Form filling is available only in the desktop app.',
    '表单填写仅在桌面应用中可用。',
    '表單填寫僅在桌面應用程式中可用。',
    '書式の記入はデスクトップアプリでのみ使えます。',
  ],
} satisfies Record<string, [string, string, string, string, string]>;

const languages: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export type FormFillMessage = keyof typeof messages;

export function formFillText(
  language: Language,
  key: FormFillMessage,
  values?: Record<string, string | number>,
) {
  let text = messages[key][languages[language]] ?? messages[key][1];
  if (values)
    for (const [name, value] of Object.entries(values))
      text = text.replaceAll(`{${name}}`, String(value));
  return text;
}
