extends Node3D
class_name Door


var gameData = preload("res://Resources/GameData.tres")
var audioLibrary = preload("res://Resources/AudioLibrary.tres")
var audioInstance3D = preload("res://Resources/AudioInstance3D.tscn")

@export var openAngle: Vector3
@export var openOffset: Vector3
@export var audioEvent: AudioEvent
@export var handle: Node3D
@export var key: ItemData
@export var linked: Door
@export var jammed = false
var locked = false


var defaultPosition = Vector3.ZERO
var defaultRotation = Vector3.ZERO
var targetRotation = Vector3.ZERO
var openSpeed = 4.0
var isOpen = false


var handleSpeed = 10.0
var handleTarget = Vector3.ZERO
var handleMoving: bool = false


var isOccupied = false
var occupiedTime = 5.0
var occupiedTimer = 0.0


var animationTime = 0.0

func _ready():
	animationTime = 0.0
	defaultPosition = position
	defaultRotation = rotation_degrees


	if key:
		locked = true

	elif !jammed:
		var randomRoll = randi_range(0, 5)

		if randomRoll == 0:
			animationTime += 4.0
			isOpen = true

func _physics_process(delta):

	if animationTime > 0:
		animationTime -= delta


		if isOpen:
			position = lerp(position, openOffset + defaultPosition, delta * openSpeed)
			rotation_degrees = lerp(rotation_degrees, openAngle + defaultRotation, delta * openSpeed)
		else:
			position = lerp(position, defaultPosition, delta * openSpeed)
			rotation_degrees = lerp(rotation_degrees, defaultRotation, delta * openSpeed)


		if handleMoving:

			handle.rotation_degrees = handle.rotation_degrees.lerp(handleTarget, delta * handleSpeed)


			if handleTarget != Vector3.ZERO && handle.rotation_degrees.distance_to(handleTarget) < 0.5:
				handleTarget = Vector3.ZERO


			if handleTarget == Vector3.ZERO && handle.rotation_degrees.distance_to(Vector3.ZERO) < 0.5:
				handle.rotation_degrees = Vector3.ZERO
				handleMoving = false


	if isOccupied:
		occupiedTimer += delta

		if occupiedTimer > occupiedTime:
			occupiedTimer = 0.0
			isOccupied = false

func Interact():

	if key && locked:
		CheckKey()
	elif !jammed:

		if isOccupied:
			return


		isOpen = !isOpen
		animationTime += 4.0
		handleMoving = true


		if openAngle.y > 0.0: handleTarget = Vector3(0, 0, -45)
		else: handleTarget = Vector3(0, 0, 45)
		PlayDoor()

func CheckKey():

	var interface = get_tree().current_scene.get_node("/root/Map/Core/UI/Interface")


	for item in interface.inventoryGrid.get_children():

		if item.slotData.itemData.file == key.file:

			locked = false
			PlayUnlock()


			if linked:
				linked.locked = false


			interface.inventoryGrid.Pick(item)
			item.queue_free()
			break

func UpdateTooltip():
	if locked:
		gameData.tooltip = "Open with " + key.name
	elif jammed:
		gameData.tooltip = "Door [Jammed]"
	elif isOccupied:
		gameData.tooltip = "Door [Occupied]"
	else:
		if isOpen && !isOccupied:
			gameData.tooltip = "Door [Close]"
		else:
			gameData.tooltip = "Door [Open]"

func PlayDoor():
	var audio = audioInstance3D.instantiate()
	handle.add_child(audio)
	audio.PlayInstance(audioEvent, 5, 50)

func PlayUnlock():
	var audio = audioInstance3D.instantiate()
	handle.add_child(audio)
	audio.PlayInstance(audioLibrary.doorUnlock, 5, 50)
