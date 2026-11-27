-- Settings the viewer chose on the TV, such as the CRT filter. A setting
-- with no row keeps its default from the environment.

CREATE TABLE viewer_settings (
    name  TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
