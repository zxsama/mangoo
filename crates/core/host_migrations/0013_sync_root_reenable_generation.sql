-- A re-enabled root must allocate a fresh device-local binding identity.  The
-- original v2 identity trigger accidentally made that legal transition
-- impossible even when binding_generation advanced.
BEGIN IMMEDIATE;

DROP TRIGGER local_root_bindings_keep_identity;
CREATE TRIGGER local_root_bindings_keep_identity
BEFORE UPDATE OF library_uuid, root_uuid, local_binding_uuid, binding_generation
ON local_root_bindings
WHEN NEW.library_uuid != OLD.library_uuid
  OR NEW.root_uuid != OLD.root_uuid
  OR NEW.binding_generation < OLD.binding_generation
  OR (
    NEW.local_binding_uuid != OLD.local_binding_uuid
    AND NEW.binding_generation <= OLD.binding_generation
  )
BEGIN
  SELECT RAISE(ABORT, 'local root binding identity requires generation advance');
END;

UPDATE host_meta SET schema_version = 13 WHERE singleton = 1 AND schema_version < 13;

COMMIT;
