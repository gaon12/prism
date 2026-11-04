import type { Language } from '../../lib/types';

const messages = {
  title: [
    '검색용 임베딩 모델',
    'Search embedding model',
    '搜索嵌入模型',
    '搜尋嵌入模型',
    '検索用埋め込みモデル',
  ],
  description: [
    '파일 내용과 질문을 벡터로 바꾸는 로컬 검색 모델입니다. 채팅이나 문서를 작성하는 생성 AI와는 별개입니다.',
    'A local model that turns file content and queries into search vectors. It is separate from the generative AI used for chat and writing.',
    '在本机将文件内容和查询转换为搜索向量，与用于聊天和写作的生成式AI不同。',
    '在本機將檔案內容和查詢轉換為搜尋向量，與用於聊天及寫作的生成式AI分開。',
    'ファイルの内容と検索文を検索用ベクトルに変換するローカルモデルです。チャットや文書作成用の生成AIとは別のモデルです。',
  ],
  nativeOnly: [
    '모델 설치와 실행은 데스크톱 앱에서 사용할 수 있습니다.',
    'Model installation and execution are available in the desktop app.',
    '模型安装和运行可在桌面应用中使用。',
    '模型安裝及執行可在桌面應用程式中使用。',
    'モデルのインストールと実行はデスクトップアプリで利用できます。',
  ],
  platform: [
    '운영체제와 CPU 구조에 맞는 공식 패키지를 사용합니다: Windows x64, macOS Apple Silicon·Intel, Linux x64·ARM64. 아래에 이 기기에 맞는 파일을 표시합니다.',
    'Uses the official package for the OS and CPU: Windows x64, macOS Apple Silicon or Intel, and Linux x64 or ARM64. The matching file is listed below.',
    '使用匹配操作系统和CPU的官方包：Windows x64、macOS Apple Silicon或Intel、Linux x64或ARM64。下方显示本机对应文件。',
    '使用符合系統和CPU的官方套件：Windows x64、macOS Apple Silicon或Intel、Linux x64或ARM64。下方顯示本機對應檔案。',
    'OSとCPUに合う公式パッケージを使用します：Windows x64、macOS Apple Silicon・Intel、Linux x64・ARM64。対応ファイルを下に表示します。',
  ],
  unsupportedPlatform: [
    '이 운영체제·CPU 조합에 맞는 공식 런타임이 아직 없습니다. 모델 설치·실행은 지원되는 기기에서 이용하세요.',
    'No pinned official runtime is available for this OS and CPU combination yet. Install and run the model on a supported device.',
    '此操作系统和CPU组合暂无匹配的官方运行时。请在支持的设备上安装和运行模型。',
    '此作業系統和CPU組合尚無符合的官方執行環境。請在支援的裝置安裝及執行模型。',
    'このOSとCPUの組み合わせに対応する公式ランタイムはまだありません。対応端末でモデルをインストール・実行してください。',
  ],
  local: [
    '임베딩 입력은 이 기기 안에서 처리합니다. 다운로드나 실행은 버튼을 눌러 직접 시작하세요.',
    'Embedding inputs stay on this device. Downloads and execution start only when you choose an action.',
    '嵌入输入仅在本机处理。下载和运行均需点击按钮启动。',
    '嵌入輸入僅在本機處理。下載及執行皆須點選按鈕啟動。',
    '埋め込みの入力はこの端末内で処理します。ダウンロードと実行はボタンで開始してください。',
  ],
  loading: [
    '모델 상태를 확인하는 중…',
    'Checking model status…',
    '正在检查模型状态…',
    '正在檢查模型狀態…',
    'モデルの状態を確認中…',
  ],
  unknown: ['확인할 수 없음', 'Unavailable', '无法确认', '無法確認', '確認できません'],
  notInstalled: ['미설치', 'Not installed', '未安装', '未安裝', '未インストール'],
  downloading: [
    '다운로드·가져오기 중',
    'Downloading or importing',
    '正在下载或导入',
    '正在下載或匯入',
    'ダウンロード・読み込み中',
  ],
  ready: ['실행 준비됨', 'Ready to start', '已准备好运行', '已準備好執行', '実行準備完了'],
  starting: ['실행 준비 중', 'Starting', '正在启动', '正在啟動', '起動中'],
  running: ['실행 중', 'Running', '运行中', '執行中', '実行中'],
  cancelled: ['중단됨', 'Cancelled', '已取消', '已取消', 'キャンセル済み'],
  error: ['작업 실패', 'Operation failed', '操作失败', '操作失敗', '操作に失敗しました'],
  modelFile: ['모델 파일', 'Model file', '模型文件', '模型檔案', 'モデルファイル'],
  cpuRuntime: ['CPU 실행 환경', 'CPU runtime', 'CPU运行环境', 'CPU執行環境', 'CPU実行環境'],
  installed: ['설치됨', 'Installed', '已安装', '已安裝', 'インストール済み'],
  quantization: ['모델 형식', 'Model format', '模型格式', '模型格式', 'モデル形式'],
  dimensions: ['출력 차원', 'Output dimensions', '输出维度', '輸出維度', '出力次元'],
  download: [
    '공식 Q8 패키지 다운로드',
    'Download official Q8 package',
    '下载官方Q8包',
    '下載官方Q8套件',
    '公式Q8パッケージをダウンロード',
  ],
  downloadSize: [
    '모델 약 310 MB + CPU 실행 환경 약 12~25 MB입니다. 실행 환경을 풀어 저장할 공간도 필요합니다.',
    'About 310 MB for the model plus 12–25 MB for the CPU runtime. Extra space is needed to extract it.',
    '模型约310 MB，CPU运行环境约12–25 MB，解压还需要额外空间。',
    '模型約310 MB，CPU執行環境約12–25 MB，解壓還需要額外空間。',
    'モデル約310 MBとCPU実行環境約12～25 MBです。展開用にも空き容量が必要です。',
  ],
  start: ['모델 시작', 'Start model', '启动模型', '啟動模型', 'モデルを起動'],
  stop: ['모델 정지', 'Stop model', '停止模型', '停止模型', 'モデルを停止'],
  cancel: ['작업 중단', 'Cancel operation', '取消操作', '取消操作', '処理をキャンセル'],
  cancelRequested: ['중단 요청 중…', 'Cancelling…', '正在取消…', '正在取消…', 'キャンセル中…'],
  preparing: ['작업 준비 중…', 'Preparing…', '正在准备…', '正在準備…', '準備中…'],
  stopping: [
    '모델을 정지하는 중…',
    'Stopping the model…',
    '正在停止模型…',
    '正在停止模型…',
    'モデルを停止中…',
  ],
  progress: [
    '모델 파일 준비 진행 상황',
    'Model asset progress',
    '模型文件准备进度',
    '模型檔案準備進度',
    'モデルファイルの準備状況',
  ],
  unknownTotal: [
    '전체 크기 확인 중',
    'Total size unknown',
    '总大小未知',
    '總大小未知',
    '合計サイズは不明',
  ],
  startHint: [
    '모델 파일과 CPU 실행 환경을 모두 준비한 뒤 시작하세요. 시작하면 본문 색인의 의미 검색을 갱신합니다.',
    'Prepare both the model file and CPU runtime, then start. Starting refreshes semantic search for the content index.',
    '准备好模型文件和CPU运行环境后启动。启动时将更新内容索引的语义搜索。',
    '準備好模型檔案及CPU執行環境後啟動。啟動時會更新內容索引的語意搜尋。',
    'モデルファイルとCPU実行環境を用意してから起動してください。起動すると本文インデックスの意味検索を更新します。',
  ],
  stopHint: [
    '파일을 바꾸려면 실행 중인 모델을 먼저 정지하세요.',
    'Stop the running model before changing its files.',
    '更换文件前请先停止正在运行的模型。',
    '更換檔案前請先停止執行中的模型。',
    'ファイルを変更する前に、実行中のモデルを停止してください。',
  ],
  offlineImport: [
    '오프라인 설치',
    'Offline installation',
    '离线安装',
    '離線安裝',
    'オフラインインストール',
  ],
  importHint: [
    '다른 기기에서 받은 공식 GGUF 모델과 이 기기에 맞는 CPU 압축 파일을 각각 가져올 수 있습니다. 가져오기만으로 모델을 실행하지 않습니다.',
    'Import the official GGUF model and the CPU archive matching this device. Importing does not start the model.',
    '可分别导入官方GGUF模型和匹配本机的CPU压缩包。导入不会自动启动模型。',
    '可分別匯入官方GGUF模型及符合本機的CPU壓縮檔。匯入不會自動啟動模型。',
    '公式GGUFモデルとこの端末に合うCPUアーカイブを個別に読み込めます。読み込むだけでは起動しません。',
  ],
  importModel: [
    '공식 GGUF 가져오기',
    'Import official GGUF',
    '导入官方GGUF',
    '匯入官方GGUF',
    '公式GGUFを読み込む',
  ],
  importRuntime: [
    '공식 CPU 패키지 가져오기',
    'Import official CPU package',
    '导入官方CPU包',
    '匯入官方CPU套件',
    '公式CPUパッケージを読み込む',
  ],
  chooseModel: [
    'EmbeddingGemma 2 공식 GGUF 선택',
    'Choose an official EmbeddingGemma 2 GGUF',
    '选择官方EmbeddingGemma 2 GGUF',
    '選擇官方EmbeddingGemma 2 GGUF',
    '公式EmbeddingGemma 2 GGUFを選択',
  ],
  chooseRuntime: [
    '이 기기의 공식 CPU 패키지 선택 (ZIP / TAR.GZ)',
    'Choose the official CPU package for this device (ZIP / TAR.GZ)',
    '选择本机对应的官方CPU包（ZIP / TAR.GZ）',
    '選擇本機對應的官方CPU套件（ZIP / TAR.GZ）',
    'この端末用の公式CPUパッケージを選択（ZIP / TAR.GZ）',
  ],
  verification: [
    '허용한 공식 릴리스의 고정 SHA-256과 일치하는 파일만 설치·실행합니다. 파일 이름이 같아도 내용이 다르면 거부합니다.',
    'Only files matching the pinned SHA-256 hashes of supported official releases can be installed and run. Matching filenames alone are not sufficient.',
    '仅安装和运行与支持的官方版本固定SHA-256一致的文件。即使文件名相同，内容不同也会被拒绝。',
    '僅安裝及執行符合支援的官方版本固定SHA-256的檔案。即使檔名相同，內容不同也會拒絕。',
    '対応する公式リリースの固定SHA-256に一致するファイルだけをインストール・実行します。同じファイル名でも内容が異なる場合は拒否します。',
  ],
  accepted: [
    '지원 파일과 버전',
    'Supported files and versions',
    '支持的文件和版本',
    '支援的檔案及版本',
    '対応ファイルとバージョン',
  ],
  revision: ['모델 리비전', 'Model revision', '模型修订版本', '模型修訂版本', 'モデルのリビジョン'],
  officialModel: [
    'Google 모델 정보',
    'Google model information',
    'Google模型信息',
    'Google模型資訊',
    'Googleのモデル情報',
  ],
  officialGguf: [
    '공식 GGUF 받는 곳',
    'Official GGUF files',
    '官方GGUF文件',
    '官方GGUF檔案',
    '公式GGUFファイル',
  ],
  officialRuntime: [
    '공식 CPU 실행 환경 받는 곳',
    'Official CPU runtime',
    '官方CPU运行环境',
    '官方CPU執行環境',
    '公式CPU実行環境',
  ],
  refresh: ['상태 새로고침', 'Refresh status', '刷新状态', '重新整理狀態', '状態を更新'],
  details: ['오류 상세 보기', 'Error details', '查看错误详情', '查看錯誤詳細資訊', 'エラーの詳細'],
  errorGeneric: [
    '모델 작업을 완료하지 못했습니다. 아래 상세 내용을 확인하고 다시 시도하세요.',
    'The model operation could not be completed. Check the details below and try again.',
    '无法完成模型操作，请查看下方详情后重试。',
    '無法完成模型操作，請查看下方詳細資訊後再試。',
    'モデルの処理を完了できませんでした。以下の詳細を確認して、もう一度お試しください。',
  ],
  errorNetwork: [
    '파일을 내려받지 못했습니다. 연결을 확인하거나 공식 파일을 직접 가져오세요.',
    'Could not download the files. Check the connection or import the official files.',
    '无法下载文件。请检查连接或导入官方文件。',
    '無法下載檔案。請檢查連線或匯入官方檔案。',
    'ファイルをダウンロードできませんでした。接続を確認するか、公式ファイルを読み込んでください。',
  ],
  errorChecksum: [
    '파일의 크기나 SHA-256이 허용한 공식 파일과 다릅니다. 공식 파일을 다시 받으세요.',
    'The file size or SHA-256 does not match a supported official file. Download the official file again.',
    '文件大小或SHA-256与支持的官方文件不一致，请重新下载官方文件。',
    '檔案大小或SHA-256不符合支援的官方檔案，請重新下載官方檔案。',
    'ファイルのサイズまたはSHA-256が対応する公式ファイルと一致しません。公式ファイルを再度ダウンロードしてください。',
  ],
  errorVersion: [
    '지원하지 않는 모델 또는 실행 환경입니다. 아래 지원 파일과 버전을 확인하세요.',
    'This model or runtime is not supported. Check the supported files and versions below.',
    '不支持此模型或运行环境，请查看下方支持的文件和版本。',
    '不支援此模型或執行環境，請查看下方支援的檔案及版本。',
    'このモデルまたは実行環境には対応していません。以下の対応ファイルとバージョンを確認してください。',
  ],
  errorStorage: [
    '모델 파일을 읽거나 저장하지 못했습니다. 파일 권한과 저장 공간을 확인하세요.',
    'Could not read or save the model files. Check file permissions and disk space.',
    '无法读取或保存模型文件，请检查文件权限和磁盘空间。',
    '無法讀取或儲存模型檔案，請檢查檔案權限及磁碟空間。',
    'モデルファイルを読み込み・保存できませんでした。アクセス権と空き容量を確認してください。',
  ],
  errorServer: [
    '로컬 CPU 실행 환경을 시작하거나 연결하지 못했습니다. 상세 내용을 확인하세요.',
    'Could not start or connect to the local CPU runtime. Check the error details.',
    '无法启动或连接本机CPU运行环境，请查看错误详情。',
    '無法啟動或連線至本機CPU執行環境，請查看錯誤詳細資訊。',
    'ローカルCPU実行環境を起動・接続できませんでした。エラーの詳細を確認してください。',
  ],
  errorBusy: [
    '진행 중인 작업을 마치거나 모델을 정지한 뒤 다시 시도하세요.',
    'Finish the current operation or stop the model, then try again.',
    '请等待当前操作完成或停止模型后重试。',
    '請等待目前的操作完成或停止模型後再試。',
    '実行中の処理を完了するか、モデルを停止してからお試しください。',
  ],
  errorMissing: [
    '공식 모델 파일과 CPU 실행 환경을 모두 준비하세요.',
    'Prepare both the official model file and CPU runtime.',
    '请准备好官方模型文件和CPU运行环境。',
    '請準備好官方模型檔案及CPU執行環境。',
    '公式モデルファイルとCPU実行環境を両方用意してください。',
  ],
} satisfies Record<string, [string, string, string, string, string]>;

export type ModelMessageKey = keyof typeof messages;
const locales: Language[] = ['ko', 'en', 'zh-CN', 'zh-TW', 'ja'];
export function modelText(locale: Language, key: ModelMessageKey) {
  return messages[key][locales.indexOf(locale)] ?? messages[key][1];
}
export function modelErrorText(locale: Language, message: string) {
  let key: ModelMessageKey = 'errorGeneric';
  if (/cancelled/i.test(message)) key = 'cancelled';
  else if (/SHA-256|wrong size|pinned size|size mismatch/i.test(message)) key = 'errorChecksum';
  else if (/No pinned CPU runtime/i.test(message)) key = 'unsupportedPlatform';
  else if (/currently supports|runtime currently supports/i.test(message)) key = 'platform';
  else if (/already running|stop .*before|another .*operation/i.test(message)) key = 'errorBusy';
  else if (/unsupported|unexpected runtime|invalid runtime|unsafe .*archive/i.test(message))
    key = 'errorVersion';
  else if (/first|no embedding model|no .*installed|is missing/i.test(message))
    key = 'errorMissing';
  else if (/download|connection|HTTPS/i.test(message)) key = 'errorNetwork';
  else if (/server|launch|local embedding|local .*port/i.test(message)) key = 'errorServer';
  else if (
    /storage|cannot (read|open|save|create|flush|replace|extract|finish|encode)/i.test(message)
  )
    key = 'errorStorage';
  return modelText(locale, key);
}
