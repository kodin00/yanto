# Fresh start: no data migration from the TypeScript version

The Rust version doesn't import data from existing PostgreSQL installs. Operators re-create their Owner, Projects, and settings. This keeps the new schema free of legacy shapes (scrypt/bcrypt hashes, AI columns, JWT session versions), and the project has few enough installs that a migration tool isn't worth building.
