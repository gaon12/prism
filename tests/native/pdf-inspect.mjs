// Independent PDF inspection for developer fixtures; the application itself
// needs neither Python nor this parser. Never invoke a shell with a file path.
import { execFileSync } from 'node:child_process';
export function inspectPdf(path) {
  return JSON.parse(
    execFileSync(
      process.env.PRISM_TEST_PYTHON ?? 'python',
      [
        '-c',
        'import json,sys; from pypdf import PdfReader; r=PdfReader(sys.argv[1]); print(json.dumps({"pages":len(r.pages),"text":"\\n".join(p.extract_text() or "" for p in r.pages),"sizes":[[float(p.mediabox.width),float(p.mediabox.height)] for p in r.pages],"encrypted":r.is_encrypted},ensure_ascii=True))',
        path,
      ],
      { encoding: 'utf8', windowsHide: true, maxBuffer: 4 << 20 },
    ),
  );
}
