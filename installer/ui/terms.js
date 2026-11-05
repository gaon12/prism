// Terms shown by the setup before installing ("Terms of use" link, bottom-left).
//
// DRAFT: written by Claude from what Prism actually does today (see README.md). The project has
// no LICENSE yet, so the owner must review this text (and add the licence they choose) before a
// public release. Every statement here must stay true; update it when behaviour changes.
//
// Shape: language → { title, sections: [[heading, paragraph], ...] }.
// Languages without their own text fall back to English (see setup.js).
window.SETUP_TERMS = {
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
