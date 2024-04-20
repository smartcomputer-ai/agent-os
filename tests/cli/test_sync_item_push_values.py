import pytest
from aos.grit import *
from aos.wit import *
import aos.cli.sync_item as sync_item
import helpers_sync as helpers

async def test_sync_sync_from_push_value__empty_or_invalid_fails(tmp_path):
    with pytest.raises(ValueError):
        sync_items = sync_item.sync_from_push_value("", "")
    with pytest.raises(ValueError):
        sync_items = sync_item.sync_from_push_value("/", "ss")
    with pytest.raises(ValueError):
        sync_items = sync_item.sync_from_push_value("/asa/", "ss")
    with pytest.raises(ValueError):
        sync_items = sync_item.sync_from_push_value(None, "")

async def test_sync_sync_from_push_value__simple_string_value(tmp_path):
        item = sync_item.sync_from_push_value("/test", "test_value")
        assert item.dir_path == None
        assert item.file_name == None
        assert item.core_path == "/"
        assert item.item_name == "test"
        assert item.item_value == "test_value"

