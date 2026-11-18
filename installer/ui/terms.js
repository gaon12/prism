// Terms shown by the setup before installing ("Terms of use" link, bottom-left).
//
// DRAFT: written by Claude from what Prism actually does today (see README.md). The project has
// no LICENSE yet, so the owner must review this text (and add the licence they choose) before a
// public release. Every statement here must stay true; update it when behaviour changes.
//
// Shape: language → { title, sections: [[heading, paragraph], ...] }.
// Each supported setup language has the same five sections.
window.SETUP_TERMS = {
  'zh-CN': {
    title: 'Prism 使用条款和隐私说明',
    sections: [
      [
        '1. 使用软件',
        '你可以在这台电脑上安装和使用 Prism。Prism 按“原样”提供。在法律允许的范围内，开发者不作明示或默示保证，也不对使用过程中造成的数据丢失或损害负责。请自行备份重要文件。',
      ],
      [
        '2. 存储在这台电脑上的信息',
        '设置、搜索索引、剪贴板历史、文档草稿和对话仅存储在这台电脑上。剪贴板历史未加密，你可以在剪贴板面板中关闭或暂停它。你输入的 AI 服务 API 密钥保存在操作系统的安全存储中（Windows DPAPI、macOS 钥匙串等）。',
      ],
      [
        '3. 网络连接',
        'Prism 仅在以下情况下连接互联网：检查更新（可在设置中关闭自动检查）、你主动运行的网页搜索和词典查询、你启动的模型和词典包下载，以及你配置的 AI 服务。向外部 AI 服务发送问题或文档内容默认关闭，需要分别授予问题发送权限和各文件夹的权限。',
      ],
      [
        '4. 使用数据和问题报告',
        'Prism 不发送使用统计。只有在你预览并亲自发送时才创建问题报告，且不包含文档文本、文件路径、搜索词、剪贴板内容或 API 密钥。',
      ],
      [
        '5. 卸载',
        '卸载时选择“同时删除设置、剪贴板记录和索引”将删除 Prism 创建的数据；否则这些数据将保留在这台电脑上。无论是否选择此项，都会关闭登录时启动。搜索文件夹中的原始文件不会被删除。',
      ],
    ],
  },
  'zh-TW': {
    title: 'Prism 使用條款與隱私說明',
    sections: [
      [
        '1. 使用軟體',
        '你可以在這台電腦上安裝及使用 Prism。Prism 依「現狀」提供。在法律允許的範圍內，開發者不作明示或默示保證，也不對使用時造成的資料遺失或損害負責。請自行備份重要檔案。',
      ],
      [
        '2. 儲存在這台電腦上的資訊',
        '設定、搜尋索引、剪貼簿歷程、文件草稿及對話僅儲存在這台電腦上。剪貼簿歷程未加密，你可以在剪貼簿面板中關閉或暫停它。你輸入的 AI 服務 API 金鑰會存放在作業系統的安全儲存空間（Windows DPAPI、macOS 鑰匙圈等）。',
      ],
      [
        '3. 網路連線',
        'Prism 僅在下列情況連線至網際網路：檢查更新（可在設定中關閉自動檢查）、你主動執行的網頁搜尋及字典查詢、你啟動的模型與字典套件下載，以及你設定的 AI 服務。向外部 AI 服務傳送問題或文件內容預設為關閉，需要分別授予問題傳送權限及各資料夾的權限。',
      ],
      [
        '4. 使用資料與問題報告',
        'Prism 不傳送使用統計。只有在你預覽並親自傳送時才建立問題報告，且不包含文件文字、檔案路徑、搜尋詞、剪貼簿內容或 API 金鑰。',
      ],
      [
        '5. 解除安裝',
        '解除安裝時選擇「同時刪除設定、剪貼簿記錄和索引」會刪除 Prism 建立的資料；否則資料會保留在這台電腦上。不論是否選取此項，都會關閉登入時啟動。搜尋資料夾中的原始檔案不會被刪除。',
      ],
    ],
  },
  ja: {
    title: 'Prism の利用規約とプライバシーについて',
    sections: [
      [
        '1. ソフトウェアの利用',
        'この PC に Prism をインストールして利用できます。Prism は「現状のまま」提供されます。法律で認められる範囲で、開発者は明示・黙示の保証を行わず、利用に伴うデータの損失や損害の責任を負いません。重要なファイルは別途バックアップしてください。',
      ],
      [
        '2. この PC に保存する情報',
        '設定、検索インデックス、クリップボード履歴、文書の下書き、会話はこの PC にのみ保存します。クリップボード履歴は暗号化されていません。クリップボードパネルから無効化または一時停止できます。入力した AI サービスの API キーは、OS の安全な保存領域（Windows DPAPI、macOS キーチェーンなど）に保管します。',
      ],
      [
        '3. インターネット接続',
        'Prism がインターネットに接続するのは、更新の確認（設定で自動確認を無効にできます）、自分で実行するウェブ検索と辞書検索、自分で開始するモデル・辞書パックのダウンロード、設定した AI サービスの利用時だけです。外部 AI サービスへの質問や文書内容の送信は既定で無効です。有効にするには、質問の送信と各フォルダーの権限を別々に許可する必要があります。',
      ],
      [
        '4. 利用データと問題の報告',
        'Prism は利用統計を送信しません。問題レポートは、内容を確認して自分で送信した場合にのみ作成します。文書の内容、ファイルパス、検索語、クリップボードの内容、API キーは含みません。',
      ],
      [
        '5. アンインストール',
        '削除時に「設定、クリップボード履歴、インデックスも削除」を選ぶと、Prism が作成したデータも削除します。選ばなければ、この PC に残ります。ログイン時の起動はどちらの場合も解除します。検索対象フォルダーにある元のファイルは削除しません。',
      ],
    ],
  },
  ko: {
    title: 'Prism 이용 약관 및 개인정보 안내',
    sections: [
      [
        '1. 소프트웨어 사용',
        '이 PC에 Prism을 설치해 사용할 수 있습니다. Prism은 "있는 그대로" 제공되며, 법이 허용하는 범위에서 개발자는 명시적·묵시적 보증을 하지 않고 사용 중 발생한 데이터 손실이나 손해에 책임을 지지 않습니다. 중요한 파일은 따로 백업해 두세요.',
      ],
      [
        '2. 이 PC에 저장되는 정보',
        '설정, 검색 색인, 클립보드 기록, 문서 초안, 대화 기록은 이 PC에만 저장됩니다. 클립보드 기록은 암호화되지 않으며, 클립보드 패널에서 끄거나 일시 중지할 수 있습니다. 직접 입력한 AI 서비스 API 키는 운영체제의 보안 저장소(Windows DPAPI, macOS 키체인 등)에 보관됩니다.',
      ],
      [
        '3. 인터넷 연결',
        'Prism은 다음 경우에만 인터넷에 연결합니다: 업데이트 확인(설정에서 자동 확인을 끌 수 있음), 직접 실행한 웹 검색과 사전 조회, 직접 시작한 모델·사전 팩 내려받기, 직접 설정한 AI 서비스 사용. 질문이나 문서 내용을 외부 AI 서비스로 보내는 기능은 기본적으로 꺼져 있으며, 켜려면 질문 전송 허용과 폴더별 허용을 각각 선택해야 합니다.',
      ],
      [
        '4. 사용 통계와 문제 보고',
        'Prism은 사용 통계를 자동으로 보내지 않습니다. 문제 보고는 사용자가 내용을 미리 확인하고 직접 보낼 때만 만들어지며, 문서 내용·파일 경로·검색어·클립보드 내용·API 키는 포함하지 않습니다.',
      ],
      [
        '5. 제거',
        '제거할 때 "설정, 클립보드 기록, 색인도 삭제"를 선택하면 Prism이 만든 데이터를 함께 지웁니다. 선택하지 않으면 데이터는 이 PC에 남습니다. 로그인 시 실행 등록은 함께 해제됩니다. 검색 대상으로 등록한 원래 파일은 어떤 경우에도 지우지 않습니다.',
      ],
    ],
  },
  en: {
    title: 'Prism terms of use and privacy notice',
    sections: [
      [
        '1. Using the software',
        'You may install and use Prism on this PC. Prism is provided "as is". To the extent permitted by law, the developer gives no express or implied warranty and is not liable for data loss or damage arising from its use. Keep your own backups of important files.',
      ],
      [
        '2. What stays on this PC',
        'Settings, search indexes, clipboard history, document drafts and conversations are stored only on this PC. Clipboard history is not encrypted; you can turn it off or pause it in the clipboard panel. AI service API keys you enter are kept in the operating system’s secure storage (Windows DPAPI, the macOS Keychain and similar).',
      ],
      [
        '3. Internet connections',
        'Prism connects to the internet only to check for updates (automatic checks can be turned off in Settings), for web searches and dictionary lookups you run, for model and dictionary-pack downloads you start, and for AI services you configure. Sending questions or document content to an external AI service is off by default; it needs separate permission for questions and for each folder.',
      ],
      [
        '4. Usage data and problem reports',
        'Prism does not send usage statistics. A problem report is created only when you preview and send it yourself, and it never includes document text, file paths, search queries, clipboard contents or API keys.',
      ],
      [
        '5. Uninstalling',
        'If you choose "Also delete settings, clipboard history and indexes" when uninstalling, the data Prism created is removed; otherwise it stays on this PC. Start-on-login is turned off either way. Your original files in searched folders are never deleted.',
      ],
    ],
  },
};
