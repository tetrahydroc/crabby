extends Resource
class_name ItemData

@export_group("Naming")
@export var file: String
@export var name: String

@export_group("Stats")
@export var weight := 1.0
@export var value := 1
enum Rarity { Common, Rare, Legendary }
@export var rarity = Rarity.Common
