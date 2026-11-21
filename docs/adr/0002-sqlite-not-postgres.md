# Embedded SQLite instead of PostgreSQL

The TypeScript version stores everything in a PostgreSQL container. The rewrite uses a single SQLite database (WAL mode) inside the Master's data directory: Yanto's write load is tiny, Workers hold no state, and dropping Postgres removes a container, a credential, and a backup target. Revisit only if multiple Masters ever need shared state.
