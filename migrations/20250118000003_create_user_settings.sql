-- Create user_settings table for theme and other user preferences
CREATE TABLE IF NOT EXISTS user_settings (
    user_id INTEGER PRIMARY KEY NOT NULL,
    theme_name TEXT NOT NULL DEFAULT 'light',
    custom_themes TEXT, -- JSON array of custom theme objects
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

-- Create index on user_id for faster lookups
CREATE INDEX idx_user_settings_user_id ON user_settings(user_id);
