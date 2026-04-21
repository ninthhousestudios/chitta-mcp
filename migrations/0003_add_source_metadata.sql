-- Add source and metadata columns carried over from Python chitta.
-- Both are nullable: existing rows and callers that omit them keep working.

alter table memories add column source   text  default null;
alter table memories add column metadata jsonb default null;
