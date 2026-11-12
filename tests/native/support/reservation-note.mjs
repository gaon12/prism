// Assertion-only helper for the Korean actual-model fixture, never a document
// repair or a production validator.
export function hasReservationAbsenceStatement(text) {
  if (typeof text !== 'string') return false;
  const compact = (value) => value.replace(/[\s,*_]/g, '');
  const original = /예약[^\n]{0,80}(없|미정|미기재|기재되어있지|정해지지)/;
  // Retain the original gate as a necessary condition; only reject additional
  // false positives. This is deliberately scoped to this Korean QA fixture.
  if (!original.test(compact(text))) return false;
  return text.split(/[.!?。！？\r\n]+/u).some((sentence) => {
    const statement = compact(sentence);
    if (!original.test(statement)) return false;
    if (
      /없(?:으)?면|없(?:는|을)경우|미정(?:이)?면|미기재(?:이|라)?면|(?:기재되어있지|정해지지)않(?:으)?면/u.test(
        statement,
      )
    )
      return false;
    return !/(?:명시|답|안내|작성|표기|기재|응답|설명|언급|확인|포함|말)(?:해야|하(?:세요|십시오|라|기|도록)|해(?:야|주세요))|써(?:야|주세요)/u.test(
      statement,
    );
  });
}
