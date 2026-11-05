import type { Language } from '../../lib/types';

const messages = {
  title: [
    '이미지·스캔 문서 OCR',
    'Image and scanned-document OCR',
    '图片与扫描文档OCR',
    '圖片與掃描文件OCR',
    '画像・スキャン文書のOCR',
  ],
  enabled: [
    '새 이미지·스캔 페이지의 글자 읽기',
    'Read text in new images and scanned pages',
    '识别新图片与扫描页面中的文字',
    '辨識新圖片與掃描頁面的文字',
    '新しい画像・スキャンページの文字を読む',
  ],
  privacy: [
    '본문 검색을 허용한 폴더에서만 이 기기의 OCR로 처리합니다. 끄면 새 OCR만 중지하고 기존 본문은 유지합니다. 외부 AI 전송 권한은 별도로 적용합니다.',
    'Uses local OCR only in folders allowed for content search. Turning it off stops new OCR and keeps existing text. External AI permissions apply separately.',
    '仅对允许正文搜索的文件夹使用本机OCR。关闭后停止新识别，保留已有正文。外部AI传输权限另行适用。',
    '僅對允許內文搜尋的資料夾使用本機OCR。關閉後停止新辨識，保留已有內文。外部AI傳送權限另行適用。',
    '本文検索を許可したフォルダーだけを端末内のOCRで処理します。オフにすると新しい認識のみ停止し、既存の本文は保持します。外部AIへの送信権限は別途適用されます。',
  ],
  language: ['OCR 언어', 'OCR language', 'OCR语言', 'OCR語言', 'OCRの言語'],
  auto: [
    'OS 기본 언어',
    'Operating-system default',
    '操作系统默认语言',
    '作業系統預設語言',
    'OSの既定言語',
  ],
  checking: [
    '기기의 OCR 엔진 확인 중…',
    'Checking this device’s OCR engine…',
    '正在检查本机OCR引擎…',
    '正在檢查本機OCR引擎…',
    '端末のOCRエンジンを確認中…',
  ],
  unavailable: [
    '사용 가능한 로컬 OCR 엔진이 없습니다. OS의 OCR·언어 팩 설치 상태를 확인하세요.',
    'No local OCR engine is available. Check the operating system’s OCR and language packs.',
    '没有可用的本机OCR引擎。请检查操作系统的OCR与语言包。',
    '沒有可用的本機OCR引擎。請檢查作業系統的OCR與語言套件。',
    '利用できるローカルOCRエンジンがありません。OSのOCRと言語パックを確認してください。',
  ],
  missingLanguage: [
    '선택한 OCR 언어가 설치되어 있지 않습니다. OS에 언어 팩을 설치한 뒤 다시 확인하세요.',
    'The selected OCR language is not installed. Install its operating-system language pack, then check again.',
    '所选OCR语言尚未安装。安装对应操作系统语言包后重新检查。',
    '所選OCR語言尚未安裝。安裝對應作業系統語言套件後重新檢查。',
    '選択したOCR言語がインストールされていません。OSの言語パックをインストールして再確認してください。',
  ],
  checkFailed: [
    'OCR 엔진 상태를 확인하지 못했습니다.',
    'Could not check the OCR engine.',
    '无法检查OCR引擎。',
    '無法檢查OCR引擎。',
    'OCRエンジンの状態を確認できませんでした。',
  ],
  browser: [
    'OCR은 데스크톱 앱에서 사용할 수 있습니다.',
    'OCR is available in the desktop app.',
    'OCR可在桌面应用中使用。',
    'OCR可在桌面應用程式中使用。',
    'OCRはデスクトップアプリで利用できます。',
  ],
  refresh: ['다시 확인', 'Check again', '重新检查', '重新檢查', '再確認'],
  windowsNative: [
    'Windows 내장 OCR',
    'Windows built-in OCR',
    'Windows内置OCR',
    'Windows內建OCR',
    'Windows内蔵OCR',
  ],
  macNative: ['macOS Vision', 'macOS Vision', 'macOS Vision', 'macOS Vision', 'macOS Vision'],
  tesseract: [
    '설치된 Tesseract',
    'Installed Tesseract',
    '已安装的Tesseract',
    '已安裝的Tesseract',
    'インストール済みのTesseract',
  ],
  pdfUnavailable: [
    '스캔 PDF 렌더러를 사용할 수 없습니다. 이미지 OCR만 처리할 수 있습니다.',
    'The scanned-PDF renderer is unavailable. Only image OCR can run.',
    '扫描PDF渲染器不可用，只能处理图片OCR。',
    '掃描PDF算繪器無法使用，只能處理圖片OCR。',
    'スキャンPDFのレンダラーが利用できません。画像のOCRのみ実行できます。',
  ],
} satisfies Record<string, readonly [string, string, string, string, string]>;
const index: Record<Language, number> = { ko: 0, en: 1, 'zh-CN': 2, 'zh-TW': 3, ja: 4 };
export function ocrText(language: Language, key: keyof typeof messages) {
  return messages[key][index[language]];
}
