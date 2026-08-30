-- Schema of the original Aika Online server database, kept as documentation
-- of which fields the game needs. It is NOT the schema this project uses: we
-- design our own from it (SQLite in development, MySQL in production).
--
-- All INSERT statements were stripped: the original dump came from a private
-- server that really ran and carried real account rows, e-mail addresses and
-- password hashes. Only the structure is kept here.

-- phpMyAdmin SQL Dump
-- version 5.2.1
-- https://www.phpmyadmin.net/
--
-- Host: 127.0.0.1:3306
-- Tempo de geração: 31/10/2025 às 01:52
-- Versão do servidor: 9.1.0
-- Versão do PHP: 8.3.14

SET SQL_MODE = "NO_AUTO_VALUE_ON_ZERO";
START TRANSACTION;
SET time_zone = "+00:00";


/*!40101 SET @OLD_CHARACTER_SET_CLIENT=@@CHARACTER_SET_CLIENT */;
/*!40101 SET @OLD_CHARACTER_SET_RESULTS=@@CHARACTER_SET_RESULTS */;
/*!40101 SET @OLD_COLLATION_CONNECTION=@@COLLATION_CONNECTION */;
/*!40101 SET NAMES utf8mb4 */;

--
-- Banco de dados: `aikazap7169`
--

DELIMITER $$
--
-- Procedimentos
--
DROP PROCEDURE IF EXISTS `CleanArenaQueue`$$
CREATE DEFINER=`root`@`%` PROCEDURE `CleanArenaQueue` ()   BEGIN
    -- Remove entradas antigas (mais de 30 minutos na fila)
    DELETE FROM `arena_queue` 
    WHERE `queue_time` < DATE_SUB(NOW(), INTERVAL 30 MINUTE);
    
    -- Remove entradas canceladas antigas
    DELETE FROM `arena_queue` 
    WHERE `status` = 'cancelled' AND `queue_time` < DATE_SUB(NOW(), INTERVAL 5 MINUTE);
END$$

DROP PROCEDURE IF EXISTS `FindAvailableMap`$$
CREATE DEFINER=`root`@`%` PROCEDURE `FindAvailableMap` ()   BEGIN
    SELECT `map_id`, `map_name`, `team1_x`, `team1_y`, `team2_x`, `team2_y`
    FROM `arena_maps` 
    WHERE `is_occupied` = FALSE 
    ORDER BY `map_id` 
    LIMIT 1;
END$$

DROP PROCEDURE IF EXISTS `FindQueuedPlayers`$$
CREATE DEFINER=`root`@`%` PROCEDURE `FindQueuedPlayers` ()   BEGIN
    SELECT `character_id`, `character_name`, `server_id`, `client_id`
    FROM `arena_queue` 
    WHERE `status` = 'waiting' 
    ORDER BY `queue_time` 
    LIMIT 2;
END$$

DROP PROCEDURE IF EXISTS `GetAccountByUsername`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetAccountByUsername` (IN `p_username` VARCHAR(255))   BEGIN
  SELECT 
    id,
    password_hash,
    last_token,
    last_token_creation_time,
    nation,
    isactive,
    account_status,
    account_type,
    storage_gold,
    cash,
    premium_time,
    ban_days
  FROM accounts
  WHERE username = p_username;
END$$

DROP PROCEDURE IF EXISTS `GetBuffsByOwnerCharId`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetBuffsByOwnerCharId` (IN `p_owner_charid` INT)   BEGIN
    SELECT buff_index, buff_time
    FROM buffs
    WHERE owner_charid = p_owner_charid
    LIMIT 60;
END$$

DROP PROCEDURE IF EXISTS `GetCharacterData`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetCharacterData` (IN `accountId` INT)   BEGIN
    SELECT 
        id, 
        slot, 
        numeric_errors, 
        deleted, 
        numeric_token, 
        name, 
        classinfo, 
        strength, 
        agility, 
        intelligence, 
        constitution, 
        luck, 
        status, 
        altura, 
        tronco, 
        perna, 
        corpo, 
        curhp, 
        curmp, 
        honor, 
        killpoint, 
        infamia, 
        skillpoint, 
        experience, 
        level, 
        guildindex, 
        gold, 
        creationtime, 
        numeric_token, 
        logintime, 
        speedmove, 
        rotation, 
        lastlogin, 
        loggedtime, 
        playerkill, 
        posx, 
        posy, 
        deleted, 
        delete_time, 
        active_title
    FROM 
        characters 
    WHERE 
        owner_accid = accountId
    LIMIT 3;
END$$

DROP PROCEDURE IF EXISTS `GetCharacterQuests`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetCharacterQuests` (IN `p_charid` INT)   BEGIN
    SELECT questid, isdone, req1, req2, req3, req4, req5, updated_at
    FROM quests
    WHERE charid = p_charid;
END$$

DROP PROCEDURE IF EXISTS `GetCharacterSlots`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetCharacterSlots` (IN `owner_accid` INT)   BEGIN
    SELECT id, slot
    FROM characters
    WHERE owner_accid = owner_accid
    ORDER BY slot
    LIMIT 3;
END$$

DROP PROCEDURE IF EXISTS `GetInventoryItems`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetInventoryItems` (IN `ownerID` INT)   BEGIN
    SELECT 
        slot, 
        item_id, 
        app, 
        identific, 
        effect1_index, 
        effect2_index, 
        effect3_index, 
        effect1_value, 
        effect2_value, 
        effect3_value, 
        min, 
        max, 
        refine, 
        time 
    FROM items 
    WHERE owner_id = ownerID 
    AND slot_type = 1 
    ORDER BY slot 
    LIMIT 126;
END$$

DROP PROCEDURE IF EXISTS `GetItemBarsByOwnerCharID`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetItemBarsByOwnerCharID` (IN `owner_charid` INT)   BEGIN
    SELECT slot, item
    FROM itembars
    WHERE owner_charid = owner_charid
    ORDER BY slot
    LIMIT 40;
END$$

DROP PROCEDURE IF EXISTS `GetItemBarsByOwnerPran`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetItemBarsByOwnerPran` (IN `p_owner_charid` INT)   BEGIN
    SELECT slot, item
    FROM itembars
    WHERE owner_charid = (p_owner_charid + 1024000)
    ORDER BY slot
    LIMIT 3;
END$$

DROP PROCEDURE IF EXISTS `GetItemsByOwnerIdStorage`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetItemsByOwnerIdStorage` (IN `p_owner_id` INT)   BEGIN
    SELECT 
        slot, 
        item_id, 
        app, 
        identific, 
        effect1_index, 
        effect2_index, 
        effect3_index, 
        effect1_value, 
        effect2_value, 
        effect3_value, 
        `min`, 
        `max`, 
        refine, 
        `time`
    FROM items
    WHERE owner_id = p_owner_id
      AND slot_type = 2
    ORDER BY slot
    LIMIT 86;
END$$

DROP PROCEDURE IF EXISTS `GetItemsBySlotTypeAndOwner`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetItemsBySlotTypeAndOwner` (IN `p_slot_type` INT, IN `p_owner_id` INT)   BEGIN
    SELECT slot, item_id, app, identific, effect1_index, effect2_index, 
           effect3_index, effect1_value, effect2_value, effect3_value, 
           `min`, `max`, refine, `time`
    FROM items
    WHERE slot_type = p_slot_type 
      AND owner_id = p_owner_id
    ORDER BY slot
    LIMIT 16;
END$$

DROP PROCEDURE IF EXISTS `GetItemsBySlotTypeAndOwnerLimit42`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetItemsBySlotTypeAndOwnerLimit42` (IN `p_slot_type` INT, IN `p_owner_id` INT)   BEGIN
    SELECT slot, item_id, app, identific, effect1_index, effect2_index, 
           effect3_index, effect1_value, effect2_value, effect3_value, 
           `min`, `max`, refine, `time`
    FROM items
    WHERE slot_type = p_slot_type 
      AND owner_id = p_owner_id
    ORDER BY slot
    LIMIT 42;
END$$

DROP PROCEDURE IF EXISTS `GetItemsJ`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetItemsJ` (IN `p_owner_id` INT)   BEGIN
    SELECT 
        slot, 
        item_id, 
        app, 
        identific
    FROM 
        items
    WHERE 
        owner_id = p_owner_id
        AND slot_type = 10
    ORDER BY 
        slot
    LIMIT 24;
END$$

DROP PROCEDURE IF EXISTS `GetPransByAccountId`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetPransByAccountId` (IN `p_acc_id` INT)   BEGIN
    SELECT id, item_id, name, level, class, hp, max_hp, mp, 
           max_mp, xp, def_p, def_m, food, devotion, p_cute, 
           p_smart, p_sexy, p_energetic, p_tough, p_corrupt, 
           width, chest, leg, created_at, updated_at
    FROM prans
    WHERE acc_id = p_acc_id
    ORDER BY id
    LIMIT 2;
END$$

DROP PROCEDURE IF EXISTS `GetPransByAccountIdNew`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetPransByAccountIdNew` (IN `p_account_id` INT)   BEGIN
    SELECT 
        id, 
        item_id, 
        name, 
        level, 
        class, 
        hp, 
        max_hp, 
        mp, 
        max_mp, 
        xp, 
        def_p, 
        def_m, 
        food, 
        devotion, 
        p_cute, 
        p_smart, 
        p_sexy, 
        p_energetic, 
        p_tough, 
        p_corrupt, 
        width, 
        chest, 
        leg, 
        created_at, 
        updated_at
    FROM 
        prans
    WHERE 
        acc_id = p_account_id
    ORDER BY 
        id
    LIMIT 2;
END$$

DROP PROCEDURE IF EXISTS `GetSkillsByCharacterID`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetSkillsByCharacterID` (IN `CharID` INT)   BEGIN
    SELECT slot, type, item, level
    FROM skills
    WHERE owner_charid = CharID
    ORDER BY slot
    LIMIT 60;
END$$

DROP PROCEDURE IF EXISTS `GetSkillsByOwnerAndTypePran`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetSkillsByOwnerAndTypePran` (IN `p_owner_charid` INT, IN `p_type` INT)   BEGIN
    SELECT slot, item, level
    FROM skills
    WHERE owner_charid = p_owner_charid 
      AND type = p_type
    ORDER BY slot
    LIMIT 10;
END$$

DROP PROCEDURE IF EXISTS `GetTitlesByOwner`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `GetTitlesByOwner` (IN `charid` INT)   BEGIN
    SELECT title_index, title_level, title_progress
    FROM titles
    WHERE owner_charid = charid;
END$$

DROP PROCEDURE IF EXISTS `InsertCharacter`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `InsertCharacter` (IN `owner_accid` INT, IN `name` VARCHAR(255), IN `slot` INT, IN `classinfo` INT, IN `strength` INT, IN `agility` INT, IN `intelligence` INT, IN `constitution` INT, IN `luck` INT, IN `status` INT, IN `altura` INT, IN `tronco` INT, IN `perna` INT, IN `corpo` INT, IN `experience` INT, IN `level` INT, IN `gold` INT, IN `posx` FLOAT, IN `posy` FLOAT, IN `creationtime` INT, IN `pranevcnt` INT, IN `last_diary_event` INT)   BEGIN
END$$

DROP PROCEDURE IF EXISTS `LimparElter`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `LimparElter` (IN `PlayerName` VARCHAR(255))   BEGIN
  DELETE FROM elter WHERE nome_antigo = PlayerName;
END$$

DROP PROCEDURE IF EXISTS `OccupyMap`$$
CREATE DEFINER=`root`@`%` PROCEDURE `OccupyMap` (IN `p_map_id` TINYINT, IN `p_match_id` INT)   BEGIN
    UPDATE `arena_maps` 
    SET `is_occupied` = TRUE, 
        `current_match_id` = `p_match_id`, 
        `occupied_since` = NOW() 
    WHERE `map_id` = `p_map_id`;
END$$

DROP PROCEDURE IF EXISTS `ReleaseMap`$$
CREATE DEFINER=`root`@`%` PROCEDURE `ReleaseMap` (IN `p_map_id` TINYINT)   BEGIN
    UPDATE `arena_maps` 
    SET `is_occupied` = FALSE, 
        `current_match_id` = NULL, 
        `occupied_since` = NULL 
    WHERE `map_id` = `p_map_id`;
END$$

DROP PROCEDURE IF EXISTS `save_buffs`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_buffs` (IN `p_owner_charid` INT, IN `p_buffs` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE buffs_length INT;
    DECLARE v_buff_index INT;
    DECLARE v_buff_time INT;

    -- Iniciar transação
    START TRANSACTION;

    -- Deletar buffs antigos
    DELETE FROM buffs WHERE owner_charid = p_owner_charid;

    -- Inserir novos buffs
    SET buffs_length = JSON_LENGTH(p_buffs);
    WHILE i < buffs_length DO
        SET v_buff_index = JSON_UNQUOTE(JSON_EXTRACT(p_buffs, CONCAT('$[', i, '].buff_index')));
        SET v_buff_time = JSON_UNQUOTE(JSON_EXTRACT(p_buffs, CONCAT('$[', i, '].buff_time')));


        SET i = i + 1;
    END WHILE;

    -- Commitar transação
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_cash_inventory`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_cash_inventory` (IN `p_owner_charid` INT, IN `p_items` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE items_length INT;
    DECLARE v_slot INT;
    DECLARE v_item_id INT;
    DECLARE v_app INT;
    DECLARE v_identific INT;

    -- Start the transaction
    START TRANSACTION;

    -- Delete existing items for the owner
    DELETE FROM items WHERE owner_id = p_owner_charid AND slot_type = 10;

    -- Get the length of the JSON array
    SET items_length = JSON_LENGTH(p_items);

    -- Loop through each item in the JSON array
    WHILE i < items_length DO
        -- Extract values from JSON
        SET v_slot = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].slot')));
        SET v_item_id = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].item_id')));
        SET v_app = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].app')));
        SET v_identific = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].identific')));

        -- Insert each item into the database

        -- Increment the index
        SET i = i + 1;
    END WHILE;

    -- Commit the transaction
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_items`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_items` (IN `p_owner_id` INT, IN `p_items` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE items_length INT;
    DECLARE v_slot INT;
    DECLARE v_item_id INT;
    DECLARE v_app INT;
    DECLARE v_identific INT;
    DECLARE v_effect1_index INT;
    DECLARE v_effect1_value INT;
    DECLARE v_effect2_index INT;
    DECLARE v_effect2_value INT;
    DECLARE v_effect3_index INT;
    DECLARE v_effect3_value INT;
    DECLARE v_min INT;
    DECLARE v_max INT;
    DECLARE v_refine INT;
    DECLARE v_time INT;

    -- Iniciar transação
    START TRANSACTION;

    -- Deletar itens antigos
    DELETE FROM items WHERE owner_id = p_owner_id AND slot_type = 0;

    -- Inserir novos itens
    SET items_length = JSON_LENGTH(p_items);
    WHILE i < items_length DO
        SET v_slot = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].slot')));
        SET v_item_id = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].item_id')));
        SET v_app = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].app')));
        SET v_identific = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].identific')));
        SET v_effect1_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect1_index')));
        SET v_effect1_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect1_value')));
        SET v_effect2_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect2_index')));
        SET v_effect2_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect2_value')));
        SET v_effect3_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect3_index')));
        SET v_effect3_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect3_value')));
        SET v_min = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].min')));
        SET v_max = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].max')));
        SET v_refine = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].refine')));
        SET v_time = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].time')));


        SET i = i + 1;
    END WHILE;

    -- Commitar transação
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_items_bag`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_items_bag` (IN `p_owner_charid` INT, IN `p_items` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE items_length INT;
    DECLARE v_slot INT;
    DECLARE v_item_id INT;
    DECLARE v_app INT;
    DECLARE v_identific INT;
    DECLARE v_effect1_index INT;
    DECLARE v_effect1_value INT;
    DECLARE v_effect2_index INT;
    DECLARE v_effect2_value INT;
    DECLARE v_effect3_index INT;
    DECLARE v_effect3_value INT;
    DECLARE v_min INT;
    DECLARE v_max INT;
    DECLARE v_refine INT;
    DECLARE v_time INT;

    -- Start the transaction
    START TRANSACTION;

    -- Delete existing items for the owner
    DELETE FROM items WHERE owner_id = p_owner_charid AND slot_type = 1;

    -- Get the length of the JSON array
    SET items_length = JSON_LENGTH(p_items);

    -- Loop through each item in the JSON array
    WHILE i < items_length DO
        -- Extract values from JSON
        SET v_slot = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].slot')));
        SET v_item_id = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].item_id')));
        SET v_app = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].app')));
        SET v_identific = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].identific')));
        SET v_effect1_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect1_index')));
        SET v_effect1_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect1_value')));
        SET v_effect2_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect2_index')));
        SET v_effect2_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect2_value')));
        SET v_effect3_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect3_index')));
        SET v_effect3_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect3_value')));
        SET v_min = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].min')));
        SET v_max = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].max')));
        SET v_refine = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].refine')));
        SET v_time = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].time')));

        -- Insert each item into the database

        -- Increment the index
        SET i = i + 1;
    END WHILE;

    -- Commit the transaction
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_quests`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_quests` (IN `p_owner_charid` INT, IN `p_quests` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE quests_length INT;
    DECLARE v_quest_id INT;
    DECLARE v_req1 INT;
    DECLARE v_req2 INT;
    DECLARE v_req3 INT;
    DECLARE v_req4 INT;
    DECLARE v_req5 INT;
    DECLARE v_is_done INT;
    DECLARE v_updated_at INT;
    DECLARE v_created_at INT;

    -- Iniciar transação
    START TRANSACTION;

    -- Deletar quests antigas
    DELETE FROM quests WHERE charid = p_owner_charid;

    -- Inserir novas quests
    SET quests_length = JSON_LENGTH(p_quests);
    WHILE i < quests_length DO
        SET v_quest_id = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].quest_id')));
        SET v_req1 = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].req1')));
        SET v_req2 = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].req2')));
        SET v_req3 = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].req3')));
        SET v_req4 = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].req4')));
        SET v_req5 = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].req5')));
        SET v_is_done = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].is_done')));
        SET v_updated_at = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].updated_at')));
        SET v_created_at = JSON_UNQUOTE(JSON_EXTRACT(p_quests, CONCAT('$[', i, '].created_at')));


        SET i = i + 1;
    END WHILE;

    -- Commitar transação
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_saveaccountinfo`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_saveaccountinfo` (IN `p_account_id` INT, IN `p_isactive` TINYINT, IN `p_nation` INT, IN `p_storage_gold` INT, IN `p_cash` INT, IN `p_account_status` INT, IN `p_ban_days` INT)   BEGIN
    UPDATE accounts
    SET isactive = p_isactive,
        nation = p_nation,
        storage_gold = p_storage_gold,
        cash = p_cash,
        account_status = p_account_status,
        ban_days = p_ban_days
    WHERE id = p_account_id;
END$$

DROP PROCEDURE IF EXISTS `save_savecharacterinfo`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_savecharacterinfo` (IN `p_id` INT, IN `p_curhp` INT, IN `p_curmp` INT, IN `p_honor` INT, IN `p_killpoint` INT, IN `p_infamia` INT, IN `p_skillpoint` INT, IN `p_experience` BIGINT, IN `p_level` INT, IN `p_guildindex` INT, IN `p_gold` BIGINT, IN `p_posx` INT, IN `p_posy` INT, IN `p_active_title` INT, IN `p_name` VARCHAR(255), IN `p_rotation` FLOAT, IN `p_lastlogin` BIGINT, IN `p_playerkill` INT, IN `p_classinfo` INT, IN `p_strength` INT, IN `p_agility` INT, IN `p_intelligence` INT, IN `p_constitution` INT, IN `p_luck` INT, IN `p_status` INT)   BEGIN
    UPDATE characters 
    SET curhp = p_curhp,
        curmp = p_curmp,
        honor = p_honor,
        killpoint = p_killpoint,
        infamia = p_infamia,
        skillpoint = p_skillpoint,
        experience = p_experience,
        level = p_level,
        guildindex = p_guildindex,
        gold = p_gold,
        posx = p_posx,
        posy = p_posy,
        active_title = p_active_title,
        name = p_name,
        rotation = p_rotation,
        lastlogin = p_lastlogin,
        playerkill = p_playerkill,
        classinfo = p_classinfo,
        strength = p_strength,
        agility = p_agility,
        intelligence = p_intelligence,
        constitution = p_constitution,
        luck = p_luck,
        status = p_status
    WHERE id = p_id;
END$$

DROP PROCEDURE IF EXISTS `save_saveitemsonbar`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_saveitemsonbar` (IN `p_owner_charid` INT, IN `p_itembars` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE item_count INT DEFAULT JSON_LENGTH(p_itembars);
    DECLARE p_slot INT;
    DECLARE p_item INT;
    
    -- Inicia a transação
    START TRANSACTION;
    
    -- Remove os itens antigos do jogador
    DELETE FROM itembars WHERE owner_charid = p_owner_charid;
    
    -- Insere os novos itens
    WHILE i < item_count DO
        SET p_slot = JSON_UNQUOTE(JSON_EXTRACT(p_itembars, CONCAT('$[', i, '].slot')));
        SET p_item = JSON_UNQUOTE(JSON_EXTRACT(p_itembars, CONCAT('$[', i, '].item')));
        
        
        SET i = i + 1;
    END WHILE;
    
    -- Confirma a transação
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_saveskills`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_saveskills` (IN `p_owner_charid` INT, IN `p_basics` JSON, IN `p_others` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE basics_length INT;
    DECLARE others_length INT;
    DECLARE v_slot INT;
    DECLARE v_item INT;
    DECLARE v_level INT;

    -- Iniciar transação
    START TRANSACTION;

    -- Deletar skills antigas
    DELETE FROM skills WHERE owner_charid = p_owner_charid AND type IN (1, 2);

    -- Inserir novas skills básicas
    SET basics_length = JSON_LENGTH(p_basics);
    WHILE i < basics_length DO
        SET v_slot = JSON_UNQUOTE(JSON_EXTRACT(p_basics, CONCAT('$[', i, '].slot')));
        SET v_item = JSON_UNQUOTE(JSON_EXTRACT(p_basics, CONCAT('$[', i, '].item')));
        SET v_level = JSON_UNQUOTE(JSON_EXTRACT(p_basics, CONCAT('$[', i, '].level')));

        
        SET i = i + 1;
    END WHILE;

    -- Inserir outras skills
    SET i = 0;
    SET others_length = JSON_LENGTH(p_others);
    WHILE i < others_length DO
        SET v_slot = JSON_UNQUOTE(JSON_EXTRACT(p_others, CONCAT('$[', i, '].slot')));
        SET v_item = JSON_UNQUOTE(JSON_EXTRACT(p_others, CONCAT('$[', i, '].item')));
        SET v_level = JSON_UNQUOTE(JSON_EXTRACT(p_others, CONCAT('$[', i, '].level')));

        
        SET i = i + 1;
    END WHILE;

    -- Commitar transação
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_storage_inventory`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_storage_inventory` (IN `p_owner_charid` INT, IN `p_items` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE items_length INT;
    DECLARE v_slot INT;
    DECLARE v_item_id INT;
    DECLARE v_app INT;
    DECLARE v_identific INT;
    DECLARE v_effect1_index INT;
    DECLARE v_effect1_value INT;
    DECLARE v_effect2_index INT;
    DECLARE v_effect2_value INT;
    DECLARE v_effect3_index INT;
    DECLARE v_effect3_value INT;
    DECLARE v_min INT;
    DECLARE v_max INT;
    DECLARE v_refine INT;
    DECLARE v_time INT;

    -- Start the transaction
    START TRANSACTION;

    -- Delete existing items for the owner
    DELETE FROM items WHERE owner_id = p_owner_charid AND slot_type = 2;

    -- Get the length of the JSON array
    SET items_length = JSON_LENGTH(p_items);

    -- Loop through each item in the JSON array
    WHILE i < items_length DO
        -- Extract values from JSON
        SET v_slot = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].slot')));
        SET v_item_id = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].item_id')));
        SET v_app = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].app')));
        SET v_identific = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].identific')));
        SET v_effect1_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect1_index')));
        SET v_effect1_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect1_value')));
        SET v_effect2_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect2_index')));
        SET v_effect2_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect2_value')));
        SET v_effect3_index = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect3_index')));
        SET v_effect3_value = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].effect3_value')));
        SET v_min = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].min')));
        SET v_max = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].max')));
        SET v_refine = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].refine')));
        SET v_time = JSON_UNQUOTE(JSON_EXTRACT(p_items, CONCAT('$[', i, '].time')));

        -- Insert each item into the database

        -- Increment the index
        SET i = i + 1;
    END WHILE;

    -- Commit the transaction
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `save_titles`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `save_titles` (IN `p_owner_charid` INT, IN `p_titles` JSON)   BEGIN
    DECLARE i INT DEFAULT 0;
    DECLARE titles_length INT;
    DECLARE v_title_index INT;
    DECLARE v_title_level INT;
    DECLARE v_title_progress INT;

    -- Iniciar transação
    START TRANSACTION;

    -- Deletar títulos antigos
    DELETE FROM titles WHERE owner_charid = p_owner_charid;

    -- Inserir novos títulos
    SET titles_length = JSON_LENGTH(p_titles);
    WHILE i < titles_length DO
        SET v_title_index = JSON_UNQUOTE(JSON_EXTRACT(p_titles, CONCAT('$[', i, '].title_index')));
        SET v_title_level = JSON_UNQUOTE(JSON_EXTRACT(p_titles, CONCAT('$[', i, '].title_level')));
        SET v_title_progress = JSON_UNQUOTE(JSON_EXTRACT(p_titles, CONCAT('$[', i, '].title_progress')));


        SET i = i + 1;
    END WHILE;

    -- Commitar transação
    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `sp_GetArena1v1Ranking`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `sp_GetArena1v1Ranking` (IN `p_limit` INT)   BEGIN
    SELECT 
        player_name,
        channel_id,
        wins,
        losses,
        total_matches,
        win_rate,
        ranking_points,
        last_match,
        RANK() OVER (ORDER BY ranking_points DESC, win_rate DESC, wins DESC) as ranking_position
    FROM arena1v1_stats 
    WHERE total_matches > 0
    ORDER BY ranking_points DESC, win_rate DESC, wins DESC
    LIMIT p_limit;
END$$

DROP PROCEDURE IF EXISTS `sp_InsertOrUpdateElter`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `sp_InsertOrUpdateElter` (IN `CharacterName` VARCHAR(255), IN `CharacterNation` INT, IN `StatusValue` INT, OUT `NewNation` INT)   BEGIN
    DECLARE Nation4Count INT DEFAULT 0;
    DECLARE Nation5Count INT DEFAULT 0;

    -- Obter a contagem de jogadores nas nações 4 e 5
    SELECT COUNT(*) INTO Nation4Count FROM elter WHERE nation = 4;
    SELECT COUNT(*) INTO Nation5Count FROM elter WHERE nation = 5;

    -- Determinar a nova nação
    IF ABS(Nation4Count - Nation5Count) > 1 THEN
        IF Nation4Count < Nation5Count THEN
            SET NewNation = 4;
        ELSE
            SET NewNation = 5;
        END IF;
    ELSE
        IF Nation4Count <= Nation5Count THEN
            SET NewNation = 4;
        ELSE
            SET NewNation = 5;
        END IF;
    END IF;

    -- Inserir ou atualizar o jogador na tabela

    -- Retornar a nova nação (garantir que NewNation tem valor)
    SET NewNation = IFNULL(NewNation, 0);
END$$

DROP PROCEDURE IF EXISTS `sp_UpdateArena1v1Stats`$$
CREATE DEFINER=`root`@`localhost` PROCEDURE `sp_UpdateArena1v1Stats` (IN `p_winner_name` VARCHAR(16), IN `p_winner_channel` TINYINT, IN `p_loser_name` VARCHAR(16), IN `p_loser_channel` TINYINT)   BEGIN
    DECLARE EXIT HANDLER FOR SQLEXCEPTION
    BEGIN
        ROLLBACK;
        RESIGNAL;
    END;

    START TRANSACTION;

    -- Atualizar estatísticas do vencedor

    -- Atualizar estatísticas do perdedor

    COMMIT;
END$$

DROP PROCEDURE IF EXISTS `UpdatePlayerStats`$$
CREATE DEFINER=`root`@`%` PROCEDURE `UpdatePlayerStats` (IN `p_winner_id` INT, IN `p_winner_name` VARCHAR(16), IN `p_loser_id` INT, IN `p_loser_name` VARCHAR(16), IN `p_fight_duration` INT)   BEGIN
    -- Atualizar estatísticas do vencedor
    
    -- Atualizar estatísticas do perdedor
END$$

DELIMITER ;

-- --------------------------------------------------------

--
-- Estrutura para tabela `accounts`
--

DROP TABLE IF EXISTS `accounts`;
CREATE TABLE IF NOT EXISTS `accounts` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `forum_id` int UNSIGNED DEFAULT NULL,
  `username` varchar(16) CHARACTER SET utf8mb3 COLLATE utf8mb3_bin DEFAULT NULL,
  `password_hash` varchar(32) CHARACTER SET latin1 COLLATE latin1_swedish_ci NOT NULL,
  `mail` varchar(254) CHARACTER SET latin1 COLLATE latin1_swedish_ci DEFAULT NULL,
  `last_token` varchar(32) CHARACTER SET latin1 COLLATE latin1_swedish_ci DEFAULT NULL,
  `last_token_creation_time` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `nation` int UNSIGNED DEFAULT NULL,
  `isactive` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT '0',
  `account_status` int UNSIGNED DEFAULT '0',
  `account_type` int UNSIGNED DEFAULT '0',
  `storage_gold` int UNSIGNED DEFAULT '0',
  `cash` int DEFAULT '0',
  `ip_created` varchar(255) CHARACTER SET latin1 COLLATE latin1_swedish_ci DEFAULT NULL,
  `time_created` int UNSIGNED DEFAULT NULL,
  `premium_time` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `ban_days` int UNSIGNED DEFAULT NULL,
  `playtime` int UNSIGNED DEFAULT NULL,
  `secret_question` varchar(255) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `secret_answer` varchar(255) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `discord` varchar(100) DEFAULT NULL COMMENT 'Discord do usuário no formato usuario#1234',
  PRIMARY KEY (`id`) USING BTREE,
  UNIQUE KEY `forum_id` (`forum_id`) USING BTREE,
  UNIQUE KEY `username` (`username`,`mail`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=22210 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `accounts`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `account_validate`
--

DROP TABLE IF EXISTS `account_validate`;
CREATE TABLE IF NOT EXISTS `account_validate` (
  `id` int NOT NULL AUTO_INCREMENT,
  `email` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `code` varchar(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `verified` tinyint(1) NOT NULL DEFAULT '0',
  `referrer` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `verified_at` timestamp NULL DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP,
  `created_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`) USING BTREE,
  UNIQUE KEY `email` (`email`,`code`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `auction`
--

DROP TABLE IF EXISTS `auction`;
CREATE TABLE IF NOT EXISTS `auction` (
  `AuctionId` int NOT NULL AUTO_INCREMENT,
  `Active` int NOT NULL DEFAULT '1',
  `CharacterId` int NOT NULL,
  `ItemType` int NOT NULL,
  `ItemLevel` int NOT NULL,
  `ReinforceLevel` int NOT NULL,
  `RegisterDate` datetime NOT NULL,
  `RegisterTime` int NOT NULL,
  `SellingPrice` int NOT NULL,
  `auction_itemsId` int NOT NULL,
  PRIMARY KEY (`AuctionId`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Acionadores `auction`
--
DROP TRIGGER IF EXISTS `after_auction_insert`;
DELIMITER $$
CREATE TRIGGER `after_auction_insert` AFTER INSERT ON `auction` FOR EACH ROW BEGIN
    DECLARE v_registerdate DATETIME;
    DECLARE v_registertime INT;
    DECLARE v_character_name VARCHAR(16);  -- Variável para armazenar o nome do personagem
    DECLARE v_auction_id INT;
    
    -- Atribuindo os valores da nova linha inserida
    SET v_auction_id = NEW.AuctionId;
    SET v_registerdate = NEW.registerdate;
    SET v_registertime = NEW.registertime;

    -- Calcular ExpireDate
    SET v_registerdate = DATE_ADD(v_registerdate, INTERVAL v_registertime HOUR);
    
    -- Obter o nome do personagem com base no CharacterId
    SET v_character_name = (SELECT name FROM characters WHERE id = NEW.CharacterId LIMIT 1);
    
    -- Se não encontrar o nome, atribui 'Desconhecido'
    IF v_character_name IS NULL THEN
        SET v_character_name = 'Desconhecido';
    END IF;

    -- Inserir dados na tabela vwauction_getactiveoffers
END
$$
DELIMITER ;
DROP TRIGGER IF EXISTS `after_auction_update`;
DELIMITER $$
CREATE TRIGGER `after_auction_update` AFTER UPDATE ON `auction` FOR EACH ROW BEGIN
    DECLARE v_registerdate DATETIME;
    DECLARE v_registertime INT;
    DECLARE v_character_name VARCHAR(16);  -- Variável para armazenar o nome do personagem
    DECLARE v_auction_id INT;
    DECLARE v_auction_items_id INT;

    -- Atribuindo os valores da nova linha atualizada
    SET v_auction_id = NEW.AuctionId;
    SET v_registerdate = NEW.RegisterDate;
    SET v_registertime = NEW.RegisterTime;
    SET v_auction_items_id = NEW.auction_itemsId; -- Captura o auction_itemsId

    -- Calcular ExpireDate
    SET v_registerdate = DATE_ADD(v_registerdate, INTERVAL v_registertime HOUR);
    
    -- Obter o nome do personagem atualizado
    SET v_character_name = (SELECT name FROM characters WHERE id = NEW.CharacterId LIMIT 1);
    
    -- Se não encontrar o nome, atribui 'Desconhecido'
    IF v_character_name IS NULL THEN
        SET v_character_name = 'Desconhecido';
    END IF;

    -- Atualizar os dados na tabela vwauction_getactiveoffers
    UPDATE vwauction_getactiveoffers
    SET 
        CharacterId = NEW.CharacterId,
        CharacterName = v_character_name,
        ExpireDate = v_registerdate,
        SellingPrice = NEW.SellingPrice,
        ItemId = (SELECT item_id FROM auction_items WHERE id = NEW.auction_itemsId),
        ItemLookId = (SELECT app FROM auction_items WHERE id = NEW.auction_itemsId),
        IdentificableAddOns = (SELECT identific FROM auction_items WHERE id = NEW.auction_itemsId),
        EffectId_1 = (SELECT effect1_index FROM auction_items WHERE id = NEW.auction_itemsId),
        EffectId_2 = (SELECT effect2_index FROM auction_items WHERE id = NEW.auction_itemsId),
        EffectId_3 = (SELECT effect3_index FROM auction_items WHERE id = NEW.auction_itemsId),
        EffectValue_1 = (SELECT effect1_value FROM auction_items WHERE id = NEW.auction_itemsId),
        EffectValue_2 = (SELECT effect2_value FROM auction_items WHERE id = NEW.auction_itemsId),
        EffectValue_3 = (SELECT effect3_value FROM auction_items WHERE id = NEW.auction_itemsId),
        DurabilityMin = (SELECT min FROM auction_items WHERE id = NEW.auction_itemsId),
        DurabilityMax = (SELECT max FROM auction_items WHERE id = NEW.auction_itemsId),
        Amount_Reinforce = (SELECT refine FROM auction_items WHERE id = NEW.auction_itemsId),
        ItemTime = (SELECT time FROM auction_items WHERE id = NEW.auction_itemsId),
        ItemType = NEW.ItemType,
        ItemLevel = NEW.ItemLevel,
        ReinforceLevel = NEW.ReinforceLevel,
        Active = NEW.Active
    WHERE AuctionId = v_auction_id;
END
$$
DELIMITER ;

-- --------------------------------------------------------

--
-- Estrutura para tabela `auction_items`
--

DROP TABLE IF EXISTS `auction_items`;
CREATE TABLE IF NOT EXISTS `auction_items` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `active` tinyint UNSIGNED NOT NULL DEFAULT '1',
  `item_id` int UNSIGNED NOT NULL,
  `app` int UNSIGNED NOT NULL,
  `identific` int UNSIGNED NOT NULL,
  `effect1_index` int UNSIGNED NOT NULL,
  `effect1_value` int UNSIGNED NOT NULL,
  `effect2_index` int UNSIGNED NOT NULL,
  `effect2_value` int UNSIGNED NOT NULL,
  `effect3_index` int UNSIGNED NOT NULL,
  `effect3_value` int UNSIGNED NOT NULL,
  `min` int UNSIGNED NOT NULL,
  `max` int UNSIGNED NOT NULL,
  `refine` int UNSIGNED NOT NULL,
  `time` int UNSIGNED NOT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_unicode_ci ROW_FORMAT=DYNAMIC;

--
-- Acionadores `auction_items`
--
DROP TRIGGER IF EXISTS `after_auction_delete`;
DELIMITER $$
CREATE TRIGGER `after_auction_delete` AFTER DELETE ON `auction_items` FOR EACH ROW BEGIN
    -- Remover da tabela vwauction_getactiveoffers a entrada associada ao AuctionId do item deletado
    DELETE FROM vwauction_getactiveoffers
    WHERE AuctionId = (SELECT AuctionId FROM auction WHERE auction_itemsId = OLD.id LIMIT 1);

    -- Remover da tabela auction a entrada associada ao auction_itemsId do item deletado
    DELETE FROM auction
    WHERE auction_itemsId = OLD.id;

    -- A linha da tabela auction_items já foi excluída pela própria operação DELETE
    -- Não é necessário adicionar outro DELETE em auction_items aqui.
END
$$
DELIMITER ;

-- --------------------------------------------------------

--
-- Estrutura para tabela `auto_time`
--

DROP TABLE IF EXISTS `auto_time`;
CREATE TABLE IF NOT EXISTS `auto_time` (
  `character` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `time` smallint(5) UNSIGNED ZEROFILL NOT NULL DEFAULT '03600',
  `time_used` bigint UNSIGNED NOT NULL DEFAULT '0',
  `last_free_day` datetime DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`character`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `auto_time`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `buffs`
--

DROP TABLE IF EXISTS `buffs`;
CREATE TABLE IF NOT EXISTS `buffs` (
  `buff_index` int NOT NULL,
  `buff_time` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `owner_charid` int NOT NULL,
  PRIMARY KEY (`owner_charid`,`buff_index`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `characters`
--

DROP TABLE IF EXISTS `characters`;
CREATE TABLE IF NOT EXISTS `characters` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `owner_accid` int UNSIGNED NOT NULL,
  `name` varchar(16) CHARACTER SET latin1 COLLATE latin1_swedish_ci NOT NULL,
  `slot` int UNSIGNED NOT NULL,
  `numeric_token` varchar(4) CHARACTER SET latin1 COLLATE latin1_swedish_ci DEFAULT NULL,
  `numeric_errors` int UNSIGNED DEFAULT NULL,
  `deleted` tinyint UNSIGNED DEFAULT '0',
  `speedmove` int UNSIGNED DEFAULT NULL,
  `rotation` int UNSIGNED DEFAULT NULL,
  `lastlogin` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT '1',
  `loggedtime` int UNSIGNED DEFAULT NULL,
  `playerkill` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `classinfo` int UNSIGNED NOT NULL,
  `firstlogin` int UNSIGNED DEFAULT NULL,
  `strength` int UNSIGNED NOT NULL,
  `agility` int UNSIGNED NOT NULL,
  `intelligence` int UNSIGNED NOT NULL,
  `constitution` int UNSIGNED NOT NULL,
  `luck` int UNSIGNED NOT NULL,
  `status` int UNSIGNED NOT NULL,
  `altura` int UNSIGNED NOT NULL,
  `tronco` int UNSIGNED NOT NULL,
  `perna` int UNSIGNED NOT NULL,
  `corpo` int UNSIGNED NOT NULL,
  `curhp` int UNSIGNED DEFAULT NULL,
  `curmp` int UNSIGNED DEFAULT NULL,
  `honor` int UNSIGNED DEFAULT NULL,
  `killpoint` int UNSIGNED DEFAULT NULL,
  `infamia` int UNSIGNED DEFAULT NULL,
  `skillpoint` int UNSIGNED DEFAULT NULL,
  `experience` bigint UNSIGNED NOT NULL,
  `level` int UNSIGNED NOT NULL,
  `guildindex` int UNSIGNED DEFAULT NULL,
  `gold` int UNSIGNED DEFAULT NULL,
  `posx` int UNSIGNED NOT NULL,
  `posy` int UNSIGNED NOT NULL,
  `creationtime` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `delete_time` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `logintime` int UNSIGNED DEFAULT NULL,
  `active_title` int UNSIGNED DEFAULT '0',
  `active_action` int UNSIGNED DEFAULT NULL,
  `tp_positions` varchar(64) CHARACTER SET latin1 COLLATE latin1_swedish_ci DEFAULT NULL,
  `pranevcnt` int UNSIGNED DEFAULT NULL,
  `saved_posx` int UNSIGNED DEFAULT NULL,
  `saved_posy` int UNSIGNED DEFAULT NULL,
  `last_diary_event` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `devires`
--

DROP TABLE IF EXISTS `devires`;
CREATE TABLE IF NOT EXISTS `devires` (
  `devir_id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `nation_id` int UNSIGNED NOT NULL,
  `slot1_itemid` int UNSIGNED DEFAULT NULL,
  `slot2_itemid` int UNSIGNED DEFAULT NULL,
  `slot3_itemid` int UNSIGNED DEFAULT NULL,
  `slot4_itemid` int UNSIGNED DEFAULT NULL,
  `slot5_itemid` int UNSIGNED DEFAULT NULL,
  `slot1_name` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `slot2_name` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `slot3_name` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `slot4_name` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `slot5_name` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `slot1_timecap` bigint NOT NULL DEFAULT '1',
  `slot2_timecap` bigint NOT NULL DEFAULT '1',
  `slot3_timecap` bigint NOT NULL DEFAULT '1',
  `slot4_timecap` bigint NOT NULL DEFAULT '1',
  `slot5_timecap` bigint NOT NULL DEFAULT '1',
  `slot1_able` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `slot2_able` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `slot3_able` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `slot4_able` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `slot5_able` tinyint UNSIGNED NOT NULL DEFAULT '0',
  PRIMARY KEY (`devir_id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=16 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `devires`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `donates`
--

DROP TABLE IF EXISTS `donates`;
CREATE TABLE IF NOT EXISTS `donates` (
  `id` int NOT NULL AUTO_INCREMENT,
  `account_id` int NOT NULL,
  `asaas_id` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `transaction_id` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `icoins` int NOT NULL,
  `price` decimal(10,2) NOT NULL,
  `method` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `status` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `paid` tinyint(1) NOT NULL,
  `refunded` tinyint(1) NOT NULL,
  `updated_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  `created_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`) USING BTREE,
  UNIQUE KEY `transaction_id` (`transaction_id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `donate_users`
--

DROP TABLE IF EXISTS `donate_users`;
CREATE TABLE IF NOT EXISTS `donate_users` (
  `id` int NOT NULL AUTO_INCREMENT,
  `account_id` int NOT NULL,
  `asaas_id` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `created_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`) USING BTREE,
  UNIQUE KEY `account_id` (`account_id`,`asaas_id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `elter`
--

DROP TABLE IF EXISTS `elter`;
CREATE TABLE IF NOT EXISTS `elter` (
  `id` int NOT NULL AUTO_INCREMENT,
  `nome_antigo` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `nome_novo` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `guild_antigo` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `guild_novo` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `nation_antigo` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `nation` int DEFAULT NULL,
  `kills` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `mapa` int DEFAULT NULL,
  `status` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  PRIMARY KEY (`id`,`nome_antigo`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `elter_vars`
--

DROP TABLE IF EXISTS `elter_vars`;
CREATE TABLE IF NOT EXISTS `elter_vars` (
  `id` int NOT NULL DEFAULT '1',
  `mapa` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `time_azulx` int DEFAULT NULL,
  `time_azuly` int DEFAULT NULL,
  `time_vermelhox` int DEFAULT NULL,
  `time_vermelhoy` int DEFAULT NULL,
  `kills_azul` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `kills_vermelho` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `elter_vars`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `event_battle_royale_logs`
--

DROP TABLE IF EXISTS `event_battle_royale_logs`;
CREATE TABLE IF NOT EXISTS `event_battle_royale_logs` (
  `id` int NOT NULL AUTO_INCREMENT,
  `event_name` varchar(50) DEFAULT 'Battle Royale',
  `teleported_players` int DEFAULT NULL,
  `total_entries` int DEFAULT NULL,
  `total_exits` int DEFAULT NULL,
  `active_players` int DEFAULT NULL,
  `log_time` timestamp NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`)
) ENGINE=MyISAM AUTO_INCREMENT=3536 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `event_players_victories`
--

DROP TABLE IF EXISTS `event_players_victories`;
CREATE TABLE IF NOT EXISTS `event_players_victories` (
  `AccountID` int NOT NULL COMMENT 'ID da conta associada ao personagem',
  `CharacterID` int NOT NULL COMMENT 'ID único do personagem',
  `Name` varchar(255) NOT NULL COMMENT 'Nome do jogador',
  `VictoryCount` int DEFAULT '0' COMMENT 'Número total de vitórias no evento',
  `created_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP COMMENT 'Data e hora de criação do registro',
  `updated_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT 'Data e hora da última atualização',
  PRIMARY KEY (`CharacterID`),
  KEY `idx_AccountID` (`AccountID`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci COMMENT='Tabela para registrar as vitórias dos jogadores no evento Battle Royale';

-- --------------------------------------------------------

--
-- Estrutura para tabela `event_teleported_players`
--

DROP TABLE IF EXISTS `event_teleported_players`;
CREATE TABLE IF NOT EXISTS `event_teleported_players` (
  `account_id` int NOT NULL,
  `character_id` int NOT NULL,
  `name` varchar(50) DEFAULT NULL,
  `joined_at` datetime DEFAULT CURRENT_TIMESTAMP,
  `ListIndex` int DEFAULT NULL,
  PRIMARY KEY (`character_id`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `financial_records`
--

DROP TABLE IF EXISTS `financial_records`;
CREATE TABLE IF NOT EXISTS `financial_records` (
  `id` bigint UNSIGNED NOT NULL AUTO_INCREMENT,
  `type` enum('income','expense') CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,
  `amount` decimal(10,2) NOT NULL,
  `description` text CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,
  `date` date NOT NULL,
  `created_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `founders`
--

DROP TABLE IF EXISTS `founders`;
CREATE TABLE IF NOT EXISTS `founders` (
  `id` int NOT NULL AUTO_INCREMENT,
  `idtransaction` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `name` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `dateofcomprovant` date NOT NULL,
  `valueofcomprovant` double NOT NULL,
  `validated` int NOT NULL DEFAULT '0',
  `validated_gmid` int NOT NULL DEFAULT '0',
  `coupom` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '',
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `friend_list`
--

DROP TABLE IF EXISTS `friend_list`;
CREATE TABLE IF NOT EXISTS `friend_list` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `active` tinyint NOT NULL DEFAULT '1',
  `owner_characterId` int UNSIGNED NOT NULL,
  `friend_characterId` int UNSIGNED NOT NULL,
  `registerDate` datetime NOT NULL,
  `lastUpdateDate` datetime NOT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `gm_accounts`
--

DROP TABLE IF EXISTS `gm_accounts`;
CREATE TABLE IF NOT EXISTS `gm_accounts` (
  `id` int NOT NULL AUTO_INCREMENT,
  `username` varchar(45) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `password` varchar(45) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `password_errors` int DEFAULT NULL,
  `account_status` int NOT NULL,
  `master_priv` int NOT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `gm_commands`
--

DROP TABLE IF EXISTS `gm_commands`;
CREATE TABLE IF NOT EXISTS `gm_commands` (
  `id` int NOT NULL AUTO_INCREMENT,
  `owner_gmid` int NOT NULL,
  `command_type` int NOT NULL,
  `runned` int NOT NULL DEFAULT '0',
  `command` text CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `created_at` datetime NOT NULL,
  `runned_at` datetime NOT NULL,
  `runned_by` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `target_name` varchar(45) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '',
  `target_itemid` int NOT NULL DEFAULT '0',
  `target_itemcnt` int NOT NULL DEFAULT '1',
  `refused` int NOT NULL DEFAULT '0',
  `refused_at` datetime NOT NULL,
  `reason_run` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '',
  `reason_refuse` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '',
  `reason_create` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '',
  `coupom` varchar(45) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL DEFAULT '',
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `guilds`
--

DROP TABLE IF EXISTS `guilds`;
CREATE TABLE IF NOT EXISTS `guilds` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `slot` int UNSIGNED NOT NULL,
  `name` varchar(19) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `nation` int UNSIGNED NOT NULL,
  `experience` int UNSIGNED NOT NULL,
  `level` int UNSIGNED NOT NULL,
  `totalmembers` int UNSIGNED NOT NULL,
  `bravurepoints` int UNSIGNED NOT NULL,
  `skillpoints` int UNSIGNED NOT NULL,
  `promote` int UNSIGNED NOT NULL,
  `notice1` varchar(34) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `notice2` varchar(34) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `notice3` varchar(34) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `site` varchar(38) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `rank1` int UNSIGNED NOT NULL,
  `rank2` int UNSIGNED NOT NULL,
  `rank3` int UNSIGNED NOT NULL,
  `rank4` int UNSIGNED NOT NULL,
  `rank5` int UNSIGNED NOT NULL,
  `ally_leader` int UNSIGNED NOT NULL,
  `guild_ally1_index` int UNSIGNED NOT NULL,
  `guild_ally1_name` varchar(18) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `guild_ally2_index` int UNSIGNED NOT NULL,
  `guild_ally2_name` varchar(18) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `guild_ally3_index` int UNSIGNED NOT NULL,
  `guild_ally3_name` varchar(18) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `guild_ally4_index` int UNSIGNED NOT NULL,
  `guild_ally4_name` varchar(18) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `storage_gold` int UNSIGNED NOT NULL,
  `leader_char_index` int UNSIGNED NOT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `guilds`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `guilds_players`
--

DROP TABLE IF EXISTS `guilds_players`;
CREATE TABLE IF NOT EXISTS `guilds_players` (
  `guild_index` int UNSIGNED NOT NULL,
  `char_index` int UNSIGNED NOT NULL,
  `name` varchar(20) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `player_rank` int UNSIGNED NOT NULL,
  `classinfo` int UNSIGNED NOT NULL,
  `level` int UNSIGNED NOT NULL,
  `logged` int UNSIGNED NOT NULL,
  `last_login` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `guilds_players`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `itembars`
--

DROP TABLE IF EXISTS `itembars`;
CREATE TABLE IF NOT EXISTS `itembars` (
  `owner_charid` int UNSIGNED NOT NULL,
  `slot` int UNSIGNED NOT NULL,
  `item` int UNSIGNED NOT NULL,
  PRIMARY KEY (`owner_charid`,`slot`,`item`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `items`
--

DROP TABLE IF EXISTS `items`;
CREATE TABLE IF NOT EXISTS `items` (
  `slot_type` int NOT NULL,
  `owner_id` int UNSIGNED NOT NULL,
  `slot` int UNSIGNED NOT NULL,
  `item_id` int UNSIGNED NOT NULL DEFAULT '0',
  `app` int UNSIGNED DEFAULT NULL,
  `identific` int UNSIGNED DEFAULT NULL,
  `effect1_index` int UNSIGNED DEFAULT NULL,
  `effect1_value` int UNSIGNED DEFAULT NULL,
  `effect2_index` int UNSIGNED DEFAULT NULL,
  `effect2_value` int UNSIGNED DEFAULT NULL,
  `effect3_index` int UNSIGNED DEFAULT NULL,
  `effect3_value` int UNSIGNED DEFAULT NULL,
  `min` int UNSIGNED DEFAULT NULL,
  `max` int UNSIGNED DEFAULT NULL,
  `refine` int UNSIGNED DEFAULT '1',
  `time` int UNSIGNED DEFAULT NULL,
  `owner_mail_slot` int UNSIGNED DEFAULT NULL,
  PRIMARY KEY (`slot_type`,`owner_id`,`slot`,`item_id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_unicode_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `killpoint_log`
--

DROP TABLE IF EXISTS `killpoint_log`;
CREATE TABLE IF NOT EXISTS `killpoint_log` (
  `id` int NOT NULL AUTO_INCREMENT,
  `character_id` int DEFAULT NULL,
  `character_name` varchar(255) DEFAULT NULL,
  `killpoint_snapshot` int DEFAULT NULL,
  `killpoint_difference` int DEFAULT '0',
  `collected_at` datetime DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `leilao_site`
--

DROP TABLE IF EXISTS `leilao_site`;
CREATE TABLE IF NOT EXISTS `leilao_site` (
  `AuctionId` int NOT NULL AUTO_INCREMENT,
  `CharacterId` int DEFAULT NULL,
  `CharacterName` varchar(255) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `ExpireDate` datetime DEFAULT NULL,
  `SellingPrice` int DEFAULT NULL,
  `ItemId` int DEFAULT NULL,
  `ItemLookId` int DEFAULT NULL,
  `IdentificableAddOns` int DEFAULT NULL,
  `EffectId_1` int DEFAULT NULL,
  `EffectId_2` int DEFAULT NULL,
  `EffectId_3` int DEFAULT NULL,
  `EffectValue_1` int DEFAULT NULL,
  `EffectValue_2` int DEFAULT NULL,
  `EffectValue_3` int DEFAULT NULL,
  `DurabilityMin` int DEFAULT NULL,
  `DurabilityMax` int DEFAULT NULL,
  `Amount_Reinforce` int DEFAULT NULL,
  `ItemTime` bigint DEFAULT NULL,
  `ItemType` varchar(255) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `ItemLevel` int DEFAULT NULL,
  `ReinforceLevel` int DEFAULT NULL,
  `Active` int DEFAULT NULL,
  PRIMARY KEY (`AuctionId`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=1172 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `mails`
--

DROP TABLE IF EXISTS `mails`;
CREATE TABLE IF NOT EXISTS `mails` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `active` tinyint UNSIGNED NOT NULL DEFAULT '1',
  `characterId` int UNSIGNED NOT NULL,
  `sentCharacterId` int UNSIGNED NOT NULL,
  `sentCharacterName` varchar(16) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `title` varchar(64) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `textBody` varchar(512) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `slot` int UNSIGNED NOT NULL,
  `sentGold` int UNSIGNED NOT NULL,
  `gold` int DEFAULT NULL,
  `returnDate` datetime(1) DEFAULT NULL,
  `sentDate` datetime(1) DEFAULT NULL,
  `checked` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `canReturn` tinyint UNSIGNED NOT NULL DEFAULT '1',
  `hasItems` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `isFromAuction` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `mailReturned` tinyint UNSIGNED NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `mails`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `mails_items`
--

DROP TABLE IF EXISTS `mails_items`;
CREATE TABLE IF NOT EXISTS `mails_items` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `active` tinyint UNSIGNED NOT NULL DEFAULT '1',
  `mail_id` int UNSIGNED NOT NULL,
  `slot` int UNSIGNED NOT NULL,
  `item_id` int UNSIGNED NOT NULL,
  `app` int UNSIGNED NOT NULL,
  `identific` int UNSIGNED NOT NULL,
  `effect1_index` int UNSIGNED NOT NULL,
  `effect1_value` int UNSIGNED NOT NULL,
  `effect2_index` int UNSIGNED NOT NULL,
  `effect2_value` int UNSIGNED NOT NULL,
  `effect3_index` int UNSIGNED NOT NULL,
  `effect3_value` int UNSIGNED NOT NULL,
  `min` int UNSIGNED NOT NULL,
  `max` int UNSIGNED NOT NULL,
  `refine` int UNSIGNED NOT NULL,
  `time` int UNSIGNED NOT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb3 COLLATE=utf8mb3_unicode_ci ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `mails_items`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `nations`
--

DROP TABLE IF EXISTS `nations`;
CREATE TABLE IF NOT EXISTS `nations` (
  `nation_id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `nation_name` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `channel_id` int UNSIGNED NOT NULL,
  `nation_rank` int UNSIGNED NOT NULL,
  `guild_id_marshal` int UNSIGNED NOT NULL,
  `guild_id_tactician` int UNSIGNED NOT NULL,
  `guild_id_judge` int UNSIGNED NOT NULL,
  `guild_id_treasurer` int UNSIGNED NOT NULL,
  `citizen_tax` int UNSIGNED NOT NULL,
  `visitor_tax` int UNSIGNED NOT NULL,
  `settlement` int UNSIGNED NOT NULL,
  `nation_ally` int UNSIGNED NOT NULL,
  `marechal_ally` varchar(32) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `ally_date` int UNSIGNED NOT NULL,
  `nation_gold` bigint UNSIGNED NOT NULL,
  `cerco_guildid_attack_A1` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_A2` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_A3` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_A4` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_B1` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_B2` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_B3` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_B4` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_C1` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_C2` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_C3` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_C4` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_D1` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_D2` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_D3` int UNSIGNED NOT NULL,
  `cerco_guildid_attack_D4` int UNSIGNED NOT NULL,
  PRIMARY KEY (`nation_id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=17 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `nations`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `orders`
--

DROP TABLE IF EXISTS `orders`;
CREATE TABLE IF NOT EXISTS `orders` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `username` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NOT NULL,
  `amount` decimal(10,2) NOT NULL,
  `status` enum('pending','approved','cancelled') CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci NOT NULL DEFAULT 'pending',
  `mercadopago_id` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci DEFAULT NULL,
  `created_at` datetime NOT NULL DEFAULT CURRENT_TIMESTAMP,
  `updated_at` datetime DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=2 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_general_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `other_means`
--

DROP TABLE IF EXISTS `other_means`;
CREATE TABLE IF NOT EXISTS `other_means` (
  `id` int NOT NULL AUTO_INCREMENT,
  `username` varchar(255) NOT NULL,
  `means` varchar(255) NOT NULL,
  `ip_address` varchar(45) NOT NULL,
  `created_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `password_change_log`
--

DROP TABLE IF EXISTS `password_change_log`;
CREATE TABLE IF NOT EXISTS `password_change_log` (
  `idx` int NOT NULL DEFAULT '0',
  `userid` varchar(20) CHARACTER SET latin1 COLLATE latin1_swedish_ci NOT NULL,
  `password` varchar(20) CHARACTER SET latin1 COLLATE latin1_swedish_ci NOT NULL,
  `ip` varchar(15) CHARACTER SET latin1 COLLATE latin1_swedish_ci NOT NULL DEFAULT '127.0.0.1',
  `last_change` int NOT NULL DEFAULT '1'
) ENGINE=InnoDB DEFAULT CHARSET=latin1 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `playeroriginaldata`
--

DROP TABLE IF EXISTS `playeroriginaldata`;
CREATE TABLE IF NOT EXISTS `playeroriginaldata` (
  `AccountID` int NOT NULL,
  `CharacterID` int NOT NULL,
  `OriginalNation` int NOT NULL,
  `OriginalGuildIndex` int NOT NULL,
  `TemporaryGuildIndex` int NOT NULL,
  `TemporaryNation` int NOT NULL,
  `created_at` datetime DEFAULT CURRENT_TIMESTAMP,
  `updated_at` datetime DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
  `ListIndex` int DEFAULT NULL,
  PRIMARY KEY (`CharacterID`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `playerstatus`
--

DROP TABLE IF EXISTS `playerstatus`;
CREATE TABLE IF NOT EXISTS `playerstatus` (
  `CharacterID` int NOT NULL,
  `CharacterName` varchar(255) NOT NULL,
  `DNFis` int DEFAULT '0',
  `DNMAG` int DEFAULT '0',
  `DEFFis` int DEFAULT '0',
  `DEFMAG` int DEFAULT '0',
  `BonusDMG` int DEFAULT '0',
  `Critical` int DEFAULT '0',
  `Esquiva` int DEFAULT '0',
  `Acerto` int DEFAULT '0',
  `DuploAtk` int DEFAULT '0',
  `SpeedMove` int DEFAULT '0',
  `Resistence` int DEFAULT '0',
  `HabAtk` int DEFAULT '0',
  `DamageCritical` int DEFAULT '0',
  `ResDamageCritical` int DEFAULT '0',
  `MagPenetration` int DEFAULT '0',
  `FisPenetration` int DEFAULT '0',
  `CureTax` int DEFAULT '0',
  `CritRes` int DEFAULT '0',
  `DuploRes` int DEFAULT '0',
  `ReduceCooldown` int DEFAULT '0',
  `PvPDamage` int DEFAULT '0',
  `PvPDefense` int DEFAULT '0',
  PRIMARY KEY (`CharacterID`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `player_referrals`
--

DROP TABLE IF EXISTS `player_referrals`;
CREATE TABLE IF NOT EXISTS `player_referrals` (
  `id` int NOT NULL AUTO_INCREMENT,
  `referred_player` varchar(255) NOT NULL,
  `referring_player` varchar(255) NOT NULL,
  `ip_address` varchar(45) NOT NULL,
  `created_at` timestamp NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`)
) ENGINE=MyISAM DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `prans`
--

DROP TABLE IF EXISTS `prans`;
CREATE TABLE IF NOT EXISTS `prans` (
  `id` int UNSIGNED NOT NULL AUTO_INCREMENT,
  `acc_id` int UNSIGNED NOT NULL,
  `char_id` int UNSIGNED NOT NULL,
  `item_id` int UNSIGNED DEFAULT NULL,
  `name` varchar(20) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT '',
  `food` int NOT NULL,
  `devotion` int NOT NULL,
  `p_cute` int UNSIGNED NOT NULL,
  `p_smart` int UNSIGNED NOT NULL,
  `p_sexy` int UNSIGNED NOT NULL,
  `p_energetic` int UNSIGNED NOT NULL,
  `p_tough` int UNSIGNED NOT NULL,
  `p_corrupt` int UNSIGNED NOT NULL,
  `level` int UNSIGNED NOT NULL,
  `class` int UNSIGNED NOT NULL,
  `hp` int UNSIGNED NOT NULL,
  `max_hp` int UNSIGNED NOT NULL,
  `mp` int UNSIGNED NOT NULL,
  `max_mp` int UNSIGNED NOT NULL,
  `xp` int UNSIGNED NOT NULL,
  `def_p` int UNSIGNED NOT NULL,
  `def_m` int UNSIGNED NOT NULL,
  `width` int UNSIGNED NOT NULL,
  `chest` int UNSIGNED NOT NULL,
  `leg` int UNSIGNED NOT NULL,
  `updated_at` bigint NOT NULL,
  `created_at` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `premium_account`
--

DROP TABLE IF EXISTS `premium_account`;
CREATE TABLE IF NOT EXISTS `premium_account` (
  `username` varchar(16) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `premium_time` datetime DEFAULT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `quests`
--

DROP TABLE IF EXISTS `quests`;
CREATE TABLE IF NOT EXISTS `quests` (
  `charid` int UNSIGNED NOT NULL,
  `questid` int UNSIGNED NOT NULL,
  `req1` int UNSIGNED NOT NULL DEFAULT '0',
  `req2` int UNSIGNED NOT NULL DEFAULT '0',
  `req3` int UNSIGNED NOT NULL DEFAULT '0',
  `req4` int UNSIGNED NOT NULL DEFAULT '0',
  `req5` int UNSIGNED NOT NULL DEFAULT '0',
  `isdone` tinyint UNSIGNED NOT NULL DEFAULT '0',
  `updated_at` bigint DEFAULT '0',
  `created_at` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`charid`,`questid`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `recover_password`
--

DROP TABLE IF EXISTS `recover_password`;
CREATE TABLE IF NOT EXISTS `recover_password` (
  `id` int NOT NULL AUTO_INCREMENT,
  `email` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,
  `code` varchar(40) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,
  `created_at` timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (`id`),
  UNIQUE KEY `email` (`email`,`code`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `referrers`
--

DROP TABLE IF EXISTS `referrers`;
CREATE TABLE IF NOT EXISTS `referrers` (
  `id` int NOT NULL AUTO_INCREMENT,
  `referrer` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `code` varchar(32) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci NOT NULL,
  `total_refs` int NOT NULL DEFAULT '0',
  `completed_first_donation` int NOT NULL DEFAULT '0' COMMENT 'Referências que completaram a primeira doação com esse código de referência.',
  `total_cash_raised` int NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`) USING BTREE,
  UNIQUE KEY `referrer` (`referrer`,`code`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `server`
--

DROP TABLE IF EXISTS `server`;
CREATE TABLE IF NOT EXISTS `server` (
  `nation_id` int NOT NULL AUTO_INCREMENT,
  `nation_name` varchar(64) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `nation_player_on` int NOT NULL,
  PRIMARY KEY (`nation_id`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `server_info`
--

DROP TABLE IF EXISTS `server_info`;
CREATE TABLE IF NOT EXISTS `server_info` (
  `id` int NOT NULL AUTO_INCREMENT,
  `server_id` int NOT NULL,
  `players_active` int NOT NULL,
  `mail_count` bigint NOT NULL,
  `character_count` bigint NOT NULL,
  `guild_count` int NOT NULL,
  `pran_count` int NOT NULL,
  PRIMARY KEY (`id`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=5 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

--
-- Despejando dados para a tabela `server_info`
--


-- --------------------------------------------------------

--
-- Estrutura para tabela `site_donations`
--

DROP TABLE IF EXISTS `site_donations`;
CREATE TABLE IF NOT EXISTS `site_donations` (
  `protocolo` int NOT NULL AUTO_INCREMENT,
  `account` varchar(30) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `personagem` int DEFAULT NULL,
  `quant_coins` int NOT NULL,
  `coins_bonus` int NOT NULL DEFAULT '0',
  `coins_entregues` int NOT NULL DEFAULT '0',
  `valor` decimal(11,2) NOT NULL,
  `price` decimal(11,2) NOT NULL,
  `currency` varchar(3) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `metodo_pgto` varchar(50) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci NOT NULL,
  `status` tinyint(1) NOT NULL DEFAULT '1',
  `status_real` varchar(40) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  `data` int NOT NULL,
  `ultima_alteracao` int DEFAULT NULL,
  `transaction_code` varchar(255) CHARACTER SET utf8mb3 COLLATE utf8mb3_general_ci DEFAULT NULL,
  PRIMARY KEY (`protocolo`) USING BTREE
) ENGINE=InnoDB AUTO_INCREMENT=10030 DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `skills`
--

DROP TABLE IF EXISTS `skills`;
CREATE TABLE IF NOT EXISTS `skills` (
  `owner_charid` int UNSIGNED NOT NULL,
  `slot` int UNSIGNED NOT NULL,
  `item` int UNSIGNED NOT NULL,
  `level` int UNSIGNED NOT NULL,
  `type` int UNSIGNED NOT NULL,
  PRIMARY KEY (`owner_charid`,`slot`,`type`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `titles`
--

DROP TABLE IF EXISTS `titles`;
CREATE TABLE IF NOT EXISTS `titles` (
  `owner_charid` int UNSIGNED NOT NULL,
  `title_index` int UNSIGNED NOT NULL,
  `title_level` int UNSIGNED NOT NULL DEFAULT '0',
  `title_progress` int UNSIGNED NOT NULL,
  PRIMARY KEY (`owner_charid`,`title_index`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb3 ROW_FORMAT=DYNAMIC;

-- --------------------------------------------------------

--
-- Estrutura para tabela `titulos_site`
--

DROP TABLE IF EXISTS `titulos_site`;
CREATE TABLE IF NOT EXISTS `titulos_site` (
  `owner_charid` int UNSIGNED NOT NULL,
  `title_index` int UNSIGNED NOT NULL,
  `title_level` int UNSIGNED NOT NULL DEFAULT '0',
  `title_progress` int UNSIGNED NOT NULL,
  `data` varchar(45) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  `data_esp` varchar(45) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci DEFAULT NULL,
  PRIMARY KEY (`owner_charid`,`title_index`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- --------------------------------------------------------

--
-- Estrutura para tabela `vwauction_getactiveoffers`
--

DROP TABLE IF EXISTS `vwauction_getactiveoffers`;
CREATE TABLE IF NOT EXISTS `vwauction_getactiveoffers` (
  `AuctionId` int NOT NULL AUTO_INCREMENT,
  `CharacterId` int DEFAULT NULL,
  `CharacterName` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `ExpireDate` datetime DEFAULT NULL,
  `SellingPrice` int DEFAULT NULL,
  `ItemId` int DEFAULT NULL,
  `ItemLookId` int DEFAULT NULL,
  `IdentificableAddOns` int DEFAULT NULL,
  `EffectId_1` int DEFAULT NULL,
  `EffectId_2` int DEFAULT NULL,
  `EffectId_3` int DEFAULT NULL,
  `EffectValue_1` int DEFAULT NULL,
  `EffectValue_2` int DEFAULT NULL,
  `EffectValue_3` int DEFAULT NULL,
  `DurabilityMin` int DEFAULT NULL,
  `DurabilityMax` int DEFAULT NULL,
  `Amount_Reinforce` int DEFAULT NULL,
  `ItemTime` bigint DEFAULT NULL,
  `ItemType` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL,
  `ItemLevel` int DEFAULT NULL,
  `ReinforceLevel` int DEFAULT NULL,
  `Active` int DEFAULT NULL,
  PRIMARY KEY (`AuctionId`) USING BTREE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci ROW_FORMAT=DYNAMIC;
COMMIT;

/*!40101 SET CHARACTER_SET_CLIENT=@OLD_CHARACTER_SET_CLIENT */;
/*!40101 SET CHARACTER_SET_RESULTS=@OLD_CHARACTER_SET_RESULTS */;
/*!40101 SET COLLATION_CONNECTION=@OLD_COLLATION_CONNECTION */;
