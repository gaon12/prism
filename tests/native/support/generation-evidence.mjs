// Read only the failed request from the test-owned, already stopped profile.
// Never copy an entire conversation database or inspect the user's profile.
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { join } from 'node:path';

export function documentFailureEvidence(profile, job) {
  assert.match(job.documentId, /^[a-f0-9]{32}$/);
  assert.match(job.requestId, /^[A-Za-z0-9_-]{1,96}$/);
  const documents = new DatabaseSync(join(profile, 'documents', 'documents.sqlite3'), {
    readOnly: true,
  });
  let conversation;
  try {
    conversation = documents
      .prepare('SELECT rag_conversation_id AS id FROM documents WHERE id = ?')
      .get(job.documentId);
    assert(conversation, 'Failed document is absent from the owned profile');
  } finally {
    documents.close();
  }
  const rag = new DatabaseSync(join(profile, 'rag', 'conversations.sqlite3'), { readOnly: true });
  try {
    const rows = rag
      .prepare(
        `SELECT payload FROM conversation_turns
         WHERE conversation_id = ? AND request_id = ? AND role = 'assistant'
         ORDER BY sequence LIMIT 2`,
      )
      .all(conversation.id, `doc_${job.requestId}`);
    assert.equal(
      rows.length,
      1,
      'Expected one persisted assistant response for the failed request',
    );
    assert(
      Buffer.byteLength(rows[0].payload) <= 16 * 1024 * 1024,
      'Response exceeds storage budget',
    );
    const turn = JSON.parse(rows[0].payload);
    assert.equal(turn.requestId, `doc_${job.requestId}`);
    assert.equal(turn.role, 'assistant');
    return {
      documentId: job.documentId,
      requestId: job.requestId,
      status: turn.status,
      text: turn.text,
      finishReason: turn.finishReason,
      usage: turn.usage,
      requestedModel: turn.requestedModel,
      reportedModel: turn.reportedModel,
    };
  } finally {
    rag.close();
  }
}
