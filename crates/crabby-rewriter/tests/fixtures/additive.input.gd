extends Resource
class_name WorldSave

@export var data: Dictionary = {}

func save_data(dict: Dictionary) -> void:
	data = dict

func has_key(key: String) -> bool:
	return data.has(key)
