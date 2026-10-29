-- RadioCraft reference schema copy of the radiodj3 DDL + seed baseline.
-- Verbatim from reference/schema/database.sql (HeidiSQL / MariaDB 12.2.2 dump)
-- with one exception: the stored-procedure bookkeeping block for
-- `UpdateTracks2` (reference lines 517-579) has been removed. That procedure is
-- intentionally NOT created in the database; RadioCraft performs
-- the same bookkeeping as a Rust transaction (see src/booking.rs, which ports
-- the appendix text from docs/01-database-schema.md verbatim).
-- --------------------------------------------------------
-- Host:                         127.0.0.1
-- Server version:               12.2.2-MariaDB - MariaDB Server
-- Server OS:                    Win64
-- HeidiSQL Version:             12.14.0.7165
-- --------------------------------------------------------

/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!40101 SET NAMES utf8 */;
/*!50503 SET NAMES utf8mb4 */;
/*!40103 SET @OLD_TIME_ZONE=@@TIME_ZONE */;
/*!40103 SET TIME_ZONE='+00:00' */;
/*!40014 SET @OLD_FOREIGN_KEY_CHECKS=@@FOREIGN_KEY_CHECKS, FOREIGN_KEY_CHECKS=0 */;
/*!40101 SET @OLD_SQL_MODE=@@SQL_MODE, SQL_MODE='NO_AUTO_VALUE_ON_ZERO' */;
/*!40111 SET @OLD_SQL_NOTES=@@SQL_NOTES, SQL_NOTES=0 */;


-- Dumping structure for table radiodj3.carts
DROP TABLE IF EXISTS `carts`;
CREATE TABLE IF NOT EXISTS `carts` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(250) NOT NULL,
  PRIMARY KEY (`ID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `carts` (`ID`, `name`) VALUES
(1, 'Default');

-- Dumping structure for table radiodj3.carts_list
DROP TABLE IF EXISTS `carts_list`;
CREATE TABLE IF NOT EXISTS `carts_list` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `pID` int(10) unsigned NOT NULL,
  `swID` int(10) unsigned NOT NULL,
  `swButton` int(10) unsigned NOT NULL,
  `color` varchar(100) NOT NULL,
  PRIMARY KEY (`ID`),
  KEY `fk_carts_list_pID` (`pID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.category
DROP TABLE IF EXISTS `category`;
CREATE TABLE IF NOT EXISTS `category` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(100) NOT NULL,
  PRIMARY KEY (`ID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `category` (`ID`, `name`) VALUES
(1, 'Music'),
(2, 'Sound Effects'),
(3, 'Sweepers'),
(4, 'Station IDs'),
(5, 'Jingles'),
(6, 'Promos'),
(7, 'Commercials'),
(8, 'News'),
(9, 'Interviews'),
(10, 'Radio Shows'),
(11, 'Radio Streams');

-- Dumping structure for table radiodj3.events
DROP TABLE IF EXISTS `events`;
CREATE TABLE IF NOT EXISTS `events` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `type` tinyint(2) NOT NULL,
  `time` varchar(20) NOT NULL,
  `name` varchar(200) NOT NULL,
  `date` date DEFAULT '2002-01-01',
  `day` varchar(30) DEFAULT '&',
  `hours` varchar(100) DEFAULT '&',
  `data` text DEFAULT NULL,
  `enabled` tinyint(1) DEFAULT NULL,
  `catID` int(11) NOT NULL DEFAULT 0,
  `smart` tinyint(1) NOT NULL DEFAULT 0,
  `grouped` tinyint(1) NOT NULL DEFAULT 0,
  PRIMARY KEY (`ID`),
  KEY `idx_events_catid` (`catID`),
  KEY `idx_events_smart` (`smart`),
  KEY `idx_events_enabled` (`enabled`) USING BTREE,
  KEY `idx_events_date` (`date`,`enabled`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.events_categories
DROP TABLE IF EXISTS `events_categories`;
CREATE TABLE IF NOT EXISTS `events_categories` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(200) NOT NULL,
  PRIMARY KEY (`ID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `events_categories` (`ID`, `name`) VALUES
(1, 'Default') ;

-- Dumping structure for table radiodj3.genre
DROP TABLE IF EXISTS `genre`;
CREATE TABLE IF NOT EXISTS `genre` (
  `id` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(50) NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `uc_name` (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `genre` (`id`, `name`) VALUES
(1, 'Acapella'),
(2, 'Acid'),
(3, 'Acid Jazz'),
(4, 'Acid Punk'),
(5, 'Acoustic'),
(6, 'Alternative'),
(7, 'Alternative Rock'),
(8, 'Ambient'),
(9, 'Anime'),
(10, 'Avantgarde'),
(11, 'Ballad'),
(12, 'Bass'),
(13, 'Beat'),
(14, 'Bebob'),
(15, 'Big Band'),
(16, 'Black Metal'),
(17, 'Bluegrass'),
(18, 'Blues'),
(19, 'Booty Bass'),
(20, 'BritPop'),
(21, 'Cabaret'),
(22, 'Celtic'),
(23, 'Chamber Music'),
(24, 'Chanson'),
(25, 'Chorus'),
(26, 'Christian Gangsta Rap'),
(27, 'Christian Rap'),
(28, 'Christian Rock'),
(29, 'Classic Rock'),
(30, 'Classical'),
(31, 'Club'),
(32, 'Club - House'),
(33, 'Comedy'),
(34, 'Contemporary Christian'),
(35, 'Country'),
(36, 'Crossover'),
(37, 'Cult'),
(38, 'Dance'),
(39, 'Dance Hall'),
(40, 'Darkwave'),
(41, 'Death Metal'),
(42, 'Disco'),
(43, 'Dream'),
(44, 'Drum & Bass'),
(45, 'Drum Solo'),
(46, 'Duet'),
(47, 'Easy Listening'),
(48, 'Electronic'),
(49, 'Ethnic'),
(50, 'Euro-House'),
(51, 'Euro-Techno'),
(52, 'Eurodance'),
(53, 'Fast Fusion'),
(54, 'Folk'),
(55, 'Folk-Rock'),
(56, 'Folklore'),
(57, 'Freestyle'),
(58, 'Funk'),
(59, 'Fusion'),
(60, 'Game'),
(61, 'Gangsta'),
(62, 'Goa'),
(63, 'Gospel'),
(64, 'Gothic'),
(65, 'Gothic Rock'),
(66, 'Grunge'),
(67, 'Hard Rock'),
(68, 'Hardcore'),
(69, 'Heavy Metal'),
(70, 'Hip-Hop'),
(71, 'House'),
(72, 'Humour'),
(73, 'Indie'),
(74, 'Industrial'),
(75, 'Instrumental'),
(76, 'Instrumental Pop'),
(77, 'Instrumental Rock'),
(78, 'JPop'),
(79, 'Jazz'),
(80, 'Jazz+Funk'),
(81, 'Jungle'),
(82, 'Latin'),
(83, 'Lo-Fi'),
(84, 'Meditative'),
(85, 'Merengue'),
(86, 'Metal'),
(87, 'Musical'),
(88, 'National Folk'),
(89, 'Native US'),
(90, 'Negerpunk'),
(91, 'New Age'),
(92, 'New Wave'),
(93, 'Noise'),
(94, 'Oldies'),
(95, 'Opera'),
(96, 'Other'),
(97, 'Polka'),
(98, 'Polsk Punk'),
(99, 'Pop'),
(100, 'Pop-Folk'),
(101, 'Pop/Funk'),
(102, 'Porn Groove'),
(103, 'Power Ballad'),
(104, 'Pranks'),
(105, 'Primus'),
(106, 'Progressive Rock'),
(107, 'Psychadelic'),
(108, 'Psychedelic Rock'),
(109, 'Punk'),
(110, 'Punk Rock'),
(111, 'R&B'),
(112, 'Rap'),
(113, 'Rave'),
(114, 'Reggae'),
(115, 'Retro'),
(116, 'Revival'),
(117, 'Rhythmic Soul'),
(118, 'Rock'),
(119, 'Rock & Roll'),
(120, 'Salsa'),
(121, 'Samba'),
(122, 'Satire'),
(123, 'Showtunes'),
(124, 'Ska'),
(125, 'Slow Jam'),
(126, 'Slow Rock'),
(127, 'Sonata'),
(128, 'Soul'),
(129, 'Sound Clip'),
(130, 'Soundtrack'),
(131, 'Southern Rock'),
(132, 'Space'),
(133, 'Speech'),
(134, 'Swing'),
(135, 'Symphonic Rock'),
(136, 'Symphony'),
(137, 'Synthpop'),
(138, 'Tango'),
(139, 'Techno'),
(140, 'Techno-Industrial'),
(141, 'Terror'),
(142, 'Thrash Metal'),
(143, 'Top 40'),
(144, 'Trailer'),
(145, 'Trance'),
(146, 'Tribal'),
(147, 'Trip-Hop'),
(148, 'Vocal');

-- Dumping structure for table radiodj3.history
DROP TABLE IF EXISTS `history`;
CREATE TABLE IF NOT EXISTS `history` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `trackID` int(10) unsigned NOT NULL,
  `date_played` datetime DEFAULT '2002-01-01 00:00:01',
  `song_type` tinyint(2) NOT NULL,
  `id_subcat` int(10) unsigned NOT NULL,
  `id_genre` int(10) unsigned NOT NULL,
  `duration` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `artist` varchar(200) NOT NULL,
  `original_artist` varchar(200) NOT NULL,
  `title` varchar(200) NOT NULL,
  `album` varchar(200) NOT NULL,
  `composer` varchar(200) NOT NULL,
  `label` varchar(250) NOT NULL,
  `year` varchar(4) NOT NULL DEFAULT '1900',
  `track_no` int(11) NOT NULL DEFAULT 0,
  `disc_no` int(11) NOT NULL DEFAULT 0,
  `publisher` varchar(200) NOT NULL,
  `copyright` varchar(200) NOT NULL,
  `isrc` varchar(200) NOT NULL,
  `listeners` int(10) unsigned DEFAULT NULL,
  PRIMARY KEY (`ID`),
  KEY `idx_history_date` (`date_played`),
  KEY `idx_history_track_search` (`trackID`,`date_played`),
  KEY `idx_history_artist_title` (`artist`(100),`title`(100)),
  KEY `idx_history_meta` (`song_type`,`id_subcat`,`id_genre`),
  FULLTEXT KEY `ft_history_search` (`artist`,`title`,`album`,`composer`,`publisher`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.playlists
DROP TABLE IF EXISTS `playlists`;
CREATE TABLE IF NOT EXISTS `playlists` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(200) NOT NULL,
  PRIMARY KEY (`ID`),
  UNIQUE KEY `name` (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.playlists_list
DROP TABLE IF EXISTS `playlists_list`;
CREATE TABLE IF NOT EXISTS `playlists_list` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `pID` int(10) unsigned NOT NULL,
  `sID` int(10) NOT NULL,
  `cstart` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `cnext` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `cend` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `fin` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `fout` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `swID` int(10) unsigned NOT NULL,
  `swplay` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `vtID` int(10) unsigned NOT NULL,
  `vtplay` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `swfirst` tinyint(1) NOT NULL DEFAULT 0,
  `ord` int(10) unsigned NOT NULL,
  PRIMARY KEY (`ID`),
  KEY `ID` (`ID`),
  KEY `idx_playlists_list_pid` (`pID`),
  KEY `idx_playlists_list_sid` (`sID`),
  KEY `idx_playlists_list_swid` (`swID`),
  KEY `idx_playlists_list_vtid` (`vtID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.queuelist
DROP TABLE IF EXISTS `queuelist`;
CREATE TABLE IF NOT EXISTS `queuelist` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `songID` int(11) NOT NULL DEFAULT 0,
  `ETA` datetime DEFAULT '2002-01-01 00:00:01',
  `duration` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `artist` varchar(200) NOT NULL,
  `associated_artists` varchar(250) NOT NULL DEFAULT '',
  `title` varchar(250) NOT NULL,
  `album` varchar(250) NOT NULL,
  `swID` int(11) NOT NULL DEFAULT -1,
  `swPlay` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `vtID` int(11) NOT NULL DEFAULT -1,
  `vtPlay` decimal(11,5) NOT NULL DEFAULT 0.00000,
  PRIMARY KEY (`ID`),
  KEY `idx_queuelist_songid` (`songID`),
  KEY `idx_queuelist_eta` (`ETA`),
  KEY `idx_queuelist_swid` (`swID`),
  KEY `idx_queuelist_vtid` (`vtID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.requests
DROP TABLE IF EXISTS `requests`;
CREATE TABLE IF NOT EXISTS `requests` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `songID` int(10) unsigned NOT NULL,
  `username` varchar(200) NOT NULL DEFAULT 'Anomymous',
  `userIP` varchar(50) NOT NULL,
  `message` text DEFAULT NULL,
  `requested` datetime NOT NULL,
  `played` tinyint(1) NOT NULL DEFAULT 0,
  PRIMARY KEY (`ID`),
  KEY `idx_requests_songid` (`songID`),
  KEY `idx_requests_username` (`username`),
  KEY `idx_requests_requested` (`requested`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.rotations
DROP TABLE IF EXISTS `rotations`;
CREATE TABLE IF NOT EXISTS `rotations` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `name` varchar(200) NOT NULL,
  PRIMARY KEY (`ID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `rotations` (`ID`, `name`) VALUES
(1, 'Default');

-- Dumping structure for table radiodj3.rotations_list
DROP TABLE IF EXISTS `rotations_list`;
CREATE TABLE IF NOT EXISTS `rotations_list` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `pID` int(10) unsigned NOT NULL,
  `catID` int(10) NOT NULL,
  `subID` int(10) NOT NULL,
  `genID` int(10) NOT NULL,
  `mood` varchar(250) DEFAULT NULL,
  `gender` varchar(250) DEFAULT NULL,
  `language` varchar(250) DEFAULT NULL,
  `start_type` int(11) NOT NULL DEFAULT 0,
  `end_type` int(11) NOT NULL DEFAULT 0,
  `selType` tinyint(1) NOT NULL,
  `sweeper` tinyint(1) NOT NULL,
  `repeatRule` tinyint(1) NOT NULL DEFAULT 0,
  `ord` tinyint(2) NOT NULL,
  `data` text NOT NULL,
  `track_separation` int(11) NOT NULL DEFAULT 0,
  `artist_separation` int(11) NOT NULL DEFAULT 0,
  `title_separation` int(11) NOT NULL DEFAULT 0,
  `album_separation` int(11) NOT NULL DEFAULT 0,
  `advanced` text DEFAULT NULL,
  PRIMARY KEY (`ID`),
  KEY `idx_rotations_list_pid` (`pID`),
  KEY `idx_rotations_list_catid` (`catID`),
  KEY `idx_rotations_list_subid` (`subID`),
  KEY `idx_rotations_list_genid` (`genID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.settings
DROP TABLE IF EXISTS `settings`;
CREATE TABLE IF NOT EXISTS `settings` (
  `source` varchar(150) NOT NULL,
  `setting` varchar(150) NOT NULL,
  `value` varchar(250) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.songs
DROP TABLE IF EXISTS `songs`;
CREATE TABLE IF NOT EXISTS `songs` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `path` varchar(250) NOT NULL,
  `enabled` tinyint(1) NOT NULL DEFAULT 0,
  `date_added` datetime DEFAULT '2002-01-01 00:00:01',
  `date_modified` datetime DEFAULT '2002-01-01 00:00:01',
  `date_played` datetime DEFAULT '2002-01-01 00:00:01',
  `artist_played` datetime DEFAULT '2002-01-01 00:00:01',
  `album_played` datetime DEFAULT '2002-01-01 00:00:01',
  `title_played` datetime DEFAULT '2002-01-01 00:00:01',
  `count_played` int(11) NOT NULL DEFAULT 0,
  `play_limit` int(11) NOT NULL DEFAULT 0,
  `limit_action` tinyint(1) NOT NULL DEFAULT 0,
  `start_date` datetime DEFAULT '2002-01-01 00:00:01',
  `end_date` datetime DEFAULT '2002-01-01 00:00:01',
  `startEvent` int(11) NOT NULL DEFAULT -1,
  `endEvent` int(11) NOT NULL DEFAULT -1,
  `song_type` tinyint(2) NOT NULL,
  `id_subcat` int(10) unsigned NOT NULL,
  `id_genre` int(10) unsigned NOT NULL,
  `weight` decimal(5,2) NOT NULL DEFAULT 0.00,
  `duration` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `original_duration` decimal(11,5) NOT NULL DEFAULT 0.00000,
  `cue_times` varchar(250) NOT NULL DEFAULT '&',
  `precise_cue` tinyint(1) NOT NULL DEFAULT 0,
  `fade_type` tinyint(1) NOT NULL DEFAULT 0,
  `start_type` tinyint(1) NOT NULL DEFAULT 0,
  `end_type` tinyint(1) NOT NULL DEFAULT 0,
  `mix_type` tinyint(1) NOT NULL DEFAULT 0,
  `mood` varchar(150) NOT NULL DEFAULT 'Not Set',
  `gender` varchar(150) NOT NULL DEFAULT 'Not Set',
  `lang` varchar(150) NOT NULL DEFAULT 'und',
  `rating` int(11) NOT NULL DEFAULT 0,
  `loudness` decimal(8,3) NOT NULL DEFAULT 0.000,
  `bs_loudness` decimal(8,3) NOT NULL DEFAULT 0.000,
  `bs1770` decimal(8,3) NOT NULL DEFAULT 0.000,
  `overlay` tinyint(1) NOT NULL DEFAULT 0,
  `artist` varchar(250) NOT NULL,
  `associated_artists` varchar(250) NOT NULL DEFAULT '',
  `original_artist` varchar(250) NOT NULL,
  `title` varchar(250) NOT NULL,
  `alternate_title` varchar(250) NOT NULL,
  `album` varchar(250) NOT NULL,
  `composer` varchar(250) NOT NULL,
  `label` varchar(250) NOT NULL,
  `year` varchar(4) NOT NULL DEFAULT '1900',
  `track_no` int(11) NOT NULL DEFAULT 0,
  `disc_no` int(11) NOT NULL DEFAULT 0,
  `publisher` varchar(250) NOT NULL,
  `copyright` varchar(250) NOT NULL,
  `isrc` varchar(250) NOT NULL,
  `bpm` decimal(6,1) NOT NULL DEFAULT 0.0,
  `comments` text DEFAULT NULL,
  `sweepers` varchar(250) DEFAULT NULL,
  `image` varchar(250) NOT NULL DEFAULT 'no_image.jpg',
  `buy_link` varchar(250) NOT NULL DEFAULT 'http://',
  `url1` varchar(250) NOT NULL DEFAULT 'http://',
  `url2` varchar(250) NOT NULL DEFAULT 'http://',
  `tdate_played` datetime DEFAULT '2002-01-01 00:00:01',
  `tartist_played` datetime DEFAULT '2002-01-01 00:00:01',
  `ttitle_played` datetime DEFAULT '2002-01-01 00:00:01',
  `talbum_played` datetime DEFAULT '2002-01-01 00:00:01',
  `originalmetadata` tinyint(1) NOT NULL DEFAULT 0,
  PRIMARY KEY (`ID`),
  UNIQUE KEY `idx_path` (`path`),
  KEY `idx_artist_title` (`artist`(100),`title`(100)),
  KEY `idx_album` (`album`(100)),
  KEY `idx_rotation_core` (`enabled`,`song_type`,`id_subcat`,`date_played`),
  KEY `idx_limits` (`enabled`,`play_limit`,`limit_action`,`count_played`),
  KEY `idx_genre_subcat` (`id_genre`,`id_subcat`),
  KEY `idx_auto_dj_rotation` (`id_subcat`,`enabled`,`date_played`),
  FULLTEXT KEY `ft_search` (`artist`,`title`,`album`,`composer`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- Dumping structure for table radiodj3.subcategory
DROP TABLE IF EXISTS `subcategory`;
CREATE TABLE IF NOT EXISTS `subcategory` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `parentid` int(10) unsigned NOT NULL,
  `name` varchar(100) NOT NULL,
  `sweeper_subID` int(10) NOT NULL DEFAULT 0,
  PRIMARY KEY (`ID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `subcategory` (`ID`, `parentid`, `name`) VALUES
(1, 1, '90s'),
(2, 1, '80s'),
(3, 1, 'Oldies'),
(4, 1, '2000s'),
(5, 1, 'Heavy Rotation'),
(6, 1, 'Top 40'),
(7, 2, 'Contest Effects'),
(8, 3, 'Daily Sweepers'),
(9, 3, 'Nightly Sweepers'),
(10, 4, 'Artist IDs'),
(11, 5, 'Station Jingles'),
(12, 6, 'Radio Show Promos'),
(13, 7, 'Sponsor Commercials'),
(14, 7, 'Paid Commercials'),
(15, 8, 'Daily News'),
(16, 9, 'Music Interviews'),
(17, 9, 'Political Interviews'),
(18, 10, 'My First Show'),
(19, 10, 'My Second Show'),
(20, 11, 'Other'),
(21, 11, 'Syndicated Shows'),
(22, 11, 'Syndicated News');

-- (The stored procedure that followed this point in the original dump was removed here, see header note.)

-- Dumping structure for table radiodj3.users
DROP TABLE IF EXISTS `users`;
CREATE TABLE IF NOT EXISTS `users` (
  `ID` int(10) unsigned NOT NULL AUTO_INCREMENT,
  `user` varchar(100) NOT NULL,
  `auth` varchar(250) DEFAULT NULL,
  `data` text NOT NULL,
  PRIMARY KEY (`ID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

INSERT INTO `users` (`ID`, `user`, `auth`, `data`) VALUES
(1, 'admin', '8ZCC4bp0CQxRB0NN5pzdOJG0m6L4b71/TEJ+IJG10u93m0Du94Pl15sCgkViAMv9fY150iwQQREU5TuGqa7pmw==', '');

/*!40103 SET TIME_ZONE=IFNULL(@OLD_TIME_ZONE, 'system') */;
/*!40101 SET SQL_MODE=IFNULL(@OLD_SQL_MODE, '') */;
/*!40014 SET FOREIGN_KEY_CHECKS=IFNULL(@OLD_FOREIGN_KEY_CHECKS, 1) */;
/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;
/*!40111 SET SQL_NOTES=IFNULL(@OLD_SQL_NOTES, 1) */;
