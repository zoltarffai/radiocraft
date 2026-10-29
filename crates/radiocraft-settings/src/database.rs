//! `settings_database.xml` — DB connection info and auto-backup options
//! (docs/02 §1, §A-8).

settings_struct! {
    /// Options persisted in `settings_database.xml`.
    pub struct DatabaseSettings {
        "DbServer" => db_server: String = "127.0.0.1",
        "DbName" => db_name: String = "radiodj3",
        "DbUsername" => db_username: String = "root",
        "DbPass" => db_pass: String = "changeme",
        "lastdb_update" => last_db_update: String = "0000",
        "DebugEnable" => debug_enable: bool = false,
        "AutoBackupHour" => auto_backup_hour: i32 = 22,
        "AutoBackupEnable" => auto_backup_enable: bool = false,
        "MinimizeToTray" => minimize_to_tray: bool = false,
        "BackupFolder" => backup_folder: String = "C:\\RadioDJv3\\Setup\\Backup",
        "OnlyOneInstance" => only_one_instance: bool = true,
        "StoreSettingsToDatabase" => store_settings_to_database: bool = false,
        "SSL" => ssl: bool = false,
        "dbVersion" => db_version: i32 = 2006,
    }
}

impl DatabaseSettings {
    /// The file this setting set is normally stored in.
    pub const FILE: &'static str = "settings_database.xml";
}
