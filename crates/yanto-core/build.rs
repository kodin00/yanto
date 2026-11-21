fn main() {
    // Rebuild when a migration is added or changed (sqlx::migrate! embeds them).
    println!("cargo:rerun-if-changed=migrations");
}
