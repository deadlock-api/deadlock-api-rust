create table match_salts
(
    match_id      UInt64,
    cluster_id    Nullable(UInt32),
    metadata_salt Nullable(UInt32) default 0,
    replay_salt   Nullable(UInt32),
    created_at    DateTime         default now(),
    username      Nullable(String)
)
    engine = ReplacingMergeTree ORDER BY match_id
        SETTINGS index_granularity = 8192;

INSERT INTO match_salts (match_id, cluster_id, metadata_salt, replay_salt, created_at, username)
VALUES (34826526, 152, 1830872391, 101738505, '2025-04-13 11:25:07', 'username123'),
       (34823831, 389, 97894986, 843475473, '2025-04-13 11:25:10', 'username123'),
       (34823907, 186, 1877025112, 1735315196, '2025-04-13 11:25:10', 'username123'),
       (34231017, 186, 530490419, 563238284, '2025-04-13 09:03:54', 'username123'),
       (34271618, 411, 1102212748, 191064844, '2025-04-13 11:06:27', 'username123');
