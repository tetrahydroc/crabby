extends Node3D


var gameData = preload("res://Resources/GameData.tres")
var audioInstance3D = preload("res://Resources/AudioInstance3D.tscn")


@onready var interface =$"../UI/Interface"


@onready var music =$Music
@onready var breathing =$Breathing
@onready var heartbeat =$Heartbeat
@onready var suffering =$Suffering
@onready var suffocating =$Suffocating

var breathingVolume = 0.0
var heartbeatVolume = 0.0
var suffocatingVolume = 0.0
var sufferingVolume = 0.0


@export var area05: Array[AudioStreamMP3]
@export var borderZone: Array[AudioStreamMP3]
@export var vostok: Array[AudioStreamMP3]
@export var shelter: Array[AudioStreamMP3]


var area05ClipOrder = 0
var borderZoneClipOrder = 0
var vostokClipOrder = 0
var shelterClipOrder = 0


@onready var masterBus = AudioServer.get_bus_index("Master")
@onready var ambientBus = AudioServer.get_bus_index("Ambient")
@onready var SFXBus = AudioServer.get_bus_index("SFX")
@onready var musicBus = AudioServer.get_bus_index("Music")
var masterLowPass = AudioServer.get_bus_effect(0, 0)
var ambientLowPass = AudioServer.get_bus_effect(1, 0)
var ambientAmplify = AudioServer.get_bus_effect(1, 1)
var musicAmplify = AudioServer.get_bus_effect(3, 0)
var lowPassValue


@onready var rainMaterial = preload("res://Effects/Files/MT_Rain.tres")
@onready var snowMaterial = preload("res://Effects/Files/MT_Snow.tres")
var VFXFadeMin = 0.0
var VFXFadeMax = 0.0


var mapType: String

func _ready() -> void:
	mapType = get_tree().current_scene.get_node("/root/Map").mapType


	masterLowPass.cutoff_hz = 20000
	ambientLowPass.cutoff_hz = 20000
	ambientAmplify.volume_db = linear_to_db(1)

func _physics_process(delta):
	Indoor(delta)
	Breathing(delta)
	Heartbeat(delta)
	Suffering(delta)
	Suffocating(delta)
	Music(delta)

func Indoor(delta):
	if gameData.indoor:
		lowPassValue = 2000
		VFXFadeMin = lerp(VFXFadeMin, 10.0, delta)
		VFXFadeMax = lerp(VFXFadeMax, 20.0, delta)
	else:
		lowPassValue = 20000
		VFXFadeMin = lerp(VFXFadeMin, 0.0, delta)
		VFXFadeMax = lerp(VFXFadeMax, 10.0, delta)

	rainMaterial.distance_fade_min_distance = VFXFadeMin
	rainMaterial.distance_fade_max_distance = VFXFadeMax
	snowMaterial.distance_fade_min_distance = VFXFadeMin
	snowMaterial.distance_fade_max_distance = VFXFadeMax
	ambientLowPass.cutoff_hz = lerpf(ambientLowPass.cutoff_hz, lowPassValue, delta * 2.0)

func Breathing(delta):
	if gameData.isSubmerged:
		breathingVolume = 0.0
	else:
		if gameData.bodyStamina < 50:
			breathingVolume = move_toward(breathingVolume, 1, delta * 0.2)
			breathing.volume_db = linear_to_db(breathingVolume)
		else:
			breathingVolume = move_toward(breathingVolume, 0, delta * 0.2)
			breathing.volume_db = linear_to_db(breathingVolume)

	if breathingVolume > 0 && !breathing.is_playing():
		breathing.playing = true
	elif breathingVolume == 0 && breathing.is_playing():
		breathing.playing = false

func Heartbeat(delta):
	if gameData.health < 10 ||(gameData.isSubmerged && gameData.oxygen < 50):
		heartbeatVolume = move_toward(heartbeatVolume, 1, delta * 0.2)
		heartbeat.volume_db = linear_to_db(heartbeatVolume)
	else:
		heartbeatVolume = move_toward(heartbeatVolume, 0, delta * 2.0)
		heartbeat.volume_db = linear_to_db(heartbeatVolume)

	if heartbeatVolume > 0 && !heartbeat.is_playing():
		heartbeat.playing = true
	elif heartbeatVolume == 0 && heartbeat.is_playing():
		heartbeat.playing = false

func Suffering(delta):
	if gameData.isBurning:
		sufferingVolume = move_toward(sufferingVolume, 1, delta * 1.0)
		suffering.volume_db = linear_to_db(sufferingVolume)
	else:
		sufferingVolume = move_toward(sufferingVolume, 0, delta * 1.0)
		suffering.volume_db = linear_to_db(sufferingVolume)

	if sufferingVolume > 0 && !suffering.is_playing():
		suffering.playing = true
	elif sufferingVolume == 0 && suffering.is_playing():
		suffering.playing = false

func Suffocating(delta):
	if gameData.isSubmerged && gameData.oxygen < 25:
		suffocatingVolume = move_toward(suffocatingVolume, 1, delta * 1.0)
		suffocating.volume_db = linear_to_db(suffocatingVolume)
	else:
		suffocatingVolume = move_toward(suffocatingVolume, 0, delta * 1.0)
		suffocating.volume_db = linear_to_db(suffocatingVolume)

	if suffocatingVolume > 0 && !suffocating.is_playing():
		suffocating.playing = true
	elif suffocatingVolume == 0 && suffocating.is_playing():
		suffocating.playing = false

func Music(delta):

	if interface.casetteAudio.playing && interface.casetteOverride:
		musicAmplify.volume_db = move_toward(musicAmplify.volume_db, -80.0, delta * 10.0)
	else:
		musicAmplify.volume_db = move_toward(musicAmplify.volume_db, 0.0, delta * 10.0)


	if !music.is_playing():

		if gameData.musicPreset == 1:
			return

		elif gameData.musicPreset == 2:
			if mapType == "Shelter" || mapType == "Tutorial":
				music.stream = GetRandomShelterClip()
			elif mapType == "Area 05":
				music.stream = GetRandomArea05Clip()
			elif mapType == "Border Zone":
				music.stream = GetRandomBorderZoneClip()
			elif mapType == "Vostok":
				music.stream = GetRandomVostokClip()


		elif gameData.musicPreset == 3:
			music.stream = GetRandomShelterClip()

		elif gameData.musicPreset == 4:
			music.stream = GetRandomArea05Clip()

		elif gameData.musicPreset == 5:
			music.stream = GetRandomBorderZoneClip()

		elif gameData.musicPreset == 6:
			music.stream = GetRandomVostokClip()

		music.play()

func UpdateMusic():
	if music.is_playing():
		music.stop()

func GetRandomArea05Clip():
	var randomIndex: int = randi_range(0, area05.size() - 1)
	return area05[randomIndex]

func GetRandomBorderZoneClip():
	var randomIndex: int = randi_range(0, borderZone.size() - 1)
	return borderZone[randomIndex]

func GetRandomVostokClip():
	var randomIndex: int = randi_range(0, vostok.size() - 1)
	return vostok[randomIndex]

func GetRandomShelterClip():
	var randomIndex: int = randi_range(0, shelter.size() - 1)
	return shelter[randomIndex]

func GetNextArea05Clip():
	if area05ClipOrder >= area05.size() - 1:
		area05ClipOrder = 0
	else:
		area05ClipOrder += 1
	return area05[area05ClipOrder]

func GetNextBorderZoneClip():
	if borderZoneClipOrder >= borderZone.size() - 1:
		borderZoneClipOrder = 0
	else:
		borderZoneClipOrder += 1
	return borderZone[borderZoneClipOrder]

func GetNextVostokClip():
	if vostokClipOrder >= vostok.size() - 1:
		vostokClipOrder = 0
	else:
		vostokClipOrder += 1
	return vostok[vostokClipOrder]

func GetNextShelterClip():
	if shelterClipOrder >= shelter.size() - 1:
		shelterClipOrder = 0
	else:
		shelterClipOrder += 1
	return shelter[shelterClipOrder]
