extends RigidBody3D
class_name Pickup


var gameData = preload("res://Resources/GameData.tres")
var audioLibrary = preload("res://Resources/AudioLibrary.tres")
var audioInstance2D = preload("res://Resources/AudioInstance2D.tscn")
var audioInstance3D = preload("res://Resources/AudioInstance3D.tscn")
var explosion = preload("res://Effects/Explosion.tscn")

@export var slotData: SlotData
@export var mesh: MeshInstance3D
@export var collision: CollisionShape3D
var interface

func _ready():
	inertia = Vector3(0.05, 0.05, 0.05)
	interface = get_tree().current_scene.get_node_or_null("/root/Map/Core/UI/Interface")
	Freeze()



func Interact():

	if interface.AutoStack(slotData, interface.inventoryGrid):
		interface.UpdateStats(false)
		PlayPickup()
		queue_free()

	elif interface.Create(slotData, interface.inventoryGrid, false):
		interface.UpdateStats(false)
		PlayPickup()
		queue_free()

	else:
		interface.PlayError()



func Freeze():
	gravity_scale = 1
	sleeping = true
	can_sleep = true
	freeze = true
	continuous_cd = false
	max_contacts_reported = 0
	freeze_mode = FREEZE_MODE_STATIC

func Kinematic():
	gravity_scale = 0
	sleeping = true
	can_sleep = true
	freeze = false
	continuous_cd = true
	max_contacts_reported = 1
	contact_monitor = true
	freeze_mode = FREEZE_MODE_KINEMATIC

func Unfreeze():
	gravity_scale = 1
	sleeping = false
	can_sleep = false
	freeze = false
	continuous_cd = false
	max_contacts_reported = 0
	freeze_mode = FREEZE_MODE_STATIC
	await get_tree().create_timer(1.0, false).timeout;
	can_sleep = true



func UpdateTooltip():

	if slotData.itemData.showAmount:
		gameData.tooltip = slotData.itemData.name + " [" + "x" + str(slotData.amount) + "]"
		return


	if slotData.itemData.carrier && slotData.nested.size() != 0:

		for nested in slotData.nested:

			if nested.plate:
				gameData.tooltip = slotData.itemData.name + " [" + nested.rating + "]"
				return


	gameData.tooltip = slotData.itemData.name


	if slotData.itemData.file == "Cat" && gameData.catDead: gameData.tooltip += " (RIP)"

func UpdateAttachments():

	var attachments = get_node_or_null("Attachments")


	var bullets = get_node_or_null("Bullets")


	if bullets:
		if slotData.amount == 0:
			bullets.get_child(0).hide()
			bullets.get_child(1).hide()
		elif slotData.amount == 1:
			bullets.get_child(0).hide()
			bullets.get_child(1).show()
		elif slotData.amount > 1:
			bullets.get_child(0).show()
			bullets.get_child(1).show()


	if attachments:

		if slotData.nested.size() != 0 && attachments.get_child_count() != 0:

			for nestedItem in slotData.nested:

				for attachment in attachments.get_children():

					if attachment.name == nestedItem.file:
						attachment.show()


						if nestedItem.subtype == "Optic":
							attachment.position.z += slotData.position


							if slotData.itemData.useMount && !nestedItem.hasMount:

								var mount = attachments.get_node_or_null("Mount")

								if mount:
									mount.show()



func Explode():

	var newExplosion = explosion.instantiate()
	get_tree().get_root().add_child(newExplosion)
	newExplosion.position = global_position + Vector3(0, 0.5, 0)
	newExplosion.Explode()


	queue_free()



func PlayPickup():
	var audio = audioInstance2D.instantiate()
	get_tree().get_root().add_child(audio)
	audio.PlayInstance(audioLibrary.pickup)
