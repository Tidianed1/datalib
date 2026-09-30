#!/usr/bin/env python3
"""Generate a WAL-mode SQLite database caught mid-write: `tng.db` plus
the `tng.db-wal` beside it, and no `-shm`.

Run as a Bazel genrule
(`//datalib/backend/etl/sqlite_mirror:tng_wal_db`). The first row is
checkpointed into the main file; the second exists only in the WAL. A
reader that drops the WAL sees one row, and one that replays it sees
two. That is what an app's database looks like while the app is running
(Messages, Lightroom), and on macOS a user may grant us the database and
its `-wal` but not its `-shm`.
"""

import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path


def main(out_dir: Path) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        db = Path(tmp) / "tng.db"
        conn = sqlite3.connect(db)
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("CREATE TABLE crew (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        conn.execute("INSERT INTO crew (name) VALUES ('Jean-Luc Picard')")
        conn.commit()
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        conn.execute("PRAGMA wal_autocheckpoint=0")
        conn.execute("INSERT INTO crew (name) VALUES ('Beverly Crusher')")
        conn.commit()
        # Copied while the connection is open, so the WAL is not
        # checkpointed away on close.
        shutil.copyfile(db, out_dir / "tng.db")
        shutil.copyfile(Path(f"{db}-wal"), out_dir / "tng.db-wal")
        conn.close()


if __name__ == "__main__":
    main(Path(sys.argv[1]))
