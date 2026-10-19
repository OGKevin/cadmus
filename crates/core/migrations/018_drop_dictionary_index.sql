-- The word index is rebuilt in dictionary.sqlite. Drop the child first so
-- the foreign key does not reject removing the parent.

DROP TABLE IF EXISTS dictionary_index_entry;
DROP TABLE IF EXISTS dictionary_index_meta;
