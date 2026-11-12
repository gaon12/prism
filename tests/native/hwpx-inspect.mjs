// An independent ZIP/XML reader for actual generated output, not a codec round
// trip. Python's standard library is used only by developer QA, never the app.
import { execFileSync } from 'node:child_process';
export function inspectHwpx(path) {
  const program = `
import json,sys,zipfile,xml.etree.ElementTree as ET
with zipfile.ZipFile(sys.argv[1]) as package:
    parts=package.infolist()
    assert len(parts)<=64 and sum(p.file_size for p in parts)<=32<<20
    assert parts[0].filename=='mimetype' and parts[0].compress_type==zipfile.ZIP_STORED
    assert package.read('mimetype')==b'application/hwp+zip'
    names=[p.filename for p in parts]
    assert len(names)==len(set(names))
    assert all(p.file_size<=8<<20 for p in parts)
    sections=[ET.fromstring(package.read(n)) for n in names if n.startswith('Contents/section') and n.endswith('.xml')]
    assert sections and 'Contents/header.xml' in names
    nodes=[n for s in sections for n in s.iter()]
    local=lambda n:n.tag.rsplit('}',1)[-1]
    tables=[n for n in nodes if local(n)=='tbl']
    table_rows=[[
        [''.join(n.text or '' for n in cell.iter() if local(n)=='t')
         for cell in row if local(cell)=='tc']
        for row in table if local(row)=='tr'
    ] for table in tables]
    result={'text':'\\n'.join(n.text or '' for n in nodes if local(n)=='t'),
            'parts':len(parts),'tables':len(tables),'tableRows':table_rows,
            'notes':sum(local(n)=='footNote' for n in nodes),
            'images':sum(local(n)=='pic' for n in nodes)}
    print(json.dumps(result,ensure_ascii=True))
`;
  return JSON.parse(
    execFileSync(process.env.PRISM_TEST_PYTHON ?? 'python', ['-c', program, path], {
      encoding: 'utf8',
      windowsHide: true,
      maxBuffer: 4 << 20,
    }),
  );
}
