ALTER TABLE memories ADD COLUMN memory_type text NOT NULL DEFAULT 'memory';
CREATE INDEX idx_memories_type ON memories (memory_type);
CREATE INDEX idx_memories_profile_type_record
  ON memories (profile, memory_type, record_time DESC);
