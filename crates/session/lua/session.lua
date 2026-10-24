-- Engine API: runs upstream's build mode headless the way its UI does.
--
-- Every command calls the same upstream functions as the control or tab the
-- user would use in upstream (pinned at v2.67.2), so results stay identical.
-- Translated to Rust by luagen in crates/session's build script; requests and
-- responses are JSON (dkjson, upstream's own library).

local json = require("dkjson")
local t_insert = table.insert

local api = { }

local function build()
	return main.modes.BUILD
end

-- Values of upstream tables as plain JSON-able tables

local function arrayOf(list, f)
	local out = setmetatable({ }, { __jsontype = "array" })
	for _, v in ipairs(list or { }) do
		local x = f(v)
		if x ~= nil then
			t_insert(out, x)
		end
	end
	return out
end

local function tooltipLines(tooltip)
	return arrayOf(tooltip.lines, function(line)
		if line.text then
			return { size = line.size, text = line.text }
		else
			return { separator = true }
		end
	end)
end

local function newTooltip()
	return new("Tooltip")
end

-- Controls: upstream's own controls, addressed as "<host>.<name>". Changing
-- one calls its handler, as clicking or typing would.

local function controlHosts()
	local b = build()
	local hosts = {
		build = b.controls,
		tree = b.treeTab.controls,
		skills = b.skillsTab.controls,
		items = b.itemsTab.controls,
		config = b.configTab.varControls,
		party = b.partyTab.controls,
		calcs = { },
		popup = main.popups[1] and main.popups[1].controls or { },
	}
	for _, section in ipairs(b.calcsTab.sectionList) do
		for name, control in pairs(section.controls) do
			hosts.calcs[name] = control
		end
	end
	return hosts
end

local function findControl(path)
	local host, name = path:match("^(%a+)%.(.+)$")
	local list = host and controlHosts()[host]
	local control = list and list[name]
	if not control then
		error("no control " .. tostring(path))
	end
	return control
end

local function listLabel(value)
	if type(value) == "table" then
		return value.label or ""
	end
	return tostring(value)
end

local function describeControl(control)
	local c = { class = control._className }
	c.shown = control:IsShown() and true or false
	local enabled = control:GetProperty("enabled")
	c.enabled = enabled ~= false and enabled ~= nil
	if control._className == "DropDownControl" then
		c.list = arrayOf(control.list, listLabel)
		c.sel = control.selIndex
	elseif control._className == "EditControl" or control._className == "ResizableEditControl" then
		c.text = control.buf
		c.placeholder = control.placeholder
		c.multiline = control.lineHeight ~= nil or nil
	elseif control._className == "CheckBoxControl" then
		c.state = control.state and true or false
	elseif control._className == "ButtonControl" then
		c.label = control:GetProperty("label")
	elseif control._className == "SliderControl" then
		c.val = control.val
	elseif control._className == "LabelControl" then
		c.label = control:GetProperty("label")
	end
	local label = control.label
	if type(label) == "string" and c.label == nil then
		c.label = label
	end
	if type(control.tooltipText) == "function" then
		c.tooltip = control.tooltipText()
	elseif type(control.tooltipText) == "string" then
		c.tooltip = control.tooltipText
	end
	return c
end

function api.control(req)
	local control = findControl(req.path)
	if req.sel then
		control:SetSel(req.sel)
	elseif req.text then
		control:SetText(req.text, true)
	elseif req.state ~= nil then
		control.state = req.state
		if control.changeFunc then
			control.changeFunc(control.state)
		end
	elseif req.click then
		control.onClick()
	elseif req.val then
		control:SetVal(req.val)
	end
	return { }
end

function api.controls(req)
	local out = { }
	for _, path in ipairs(req.paths) do
		local ok, control = pcall(findControl, path)
		if ok then
			out[path] = describeControl(control)
		end
	end
	return out
end

-- Popups upstream opened (confirmations, messages)

-- Controls of a host in layout order (top to bottom, left to right), with
-- the label upstream draws next to each
local function layoutControls(controls, prefix, filter)
	local out = { }
	for name, control in pairs(controls) do
		-- Labels of other controls go with them
		local labelOf = type(name) == "string" and control._className == "LabelControl" and controls[(name:gsub("Label(%d*)$", "%1"))]
		if labelOf == control then
			labelOf = nil
		end
		if type(name) == "string" and not labelOf and (not filter or filter(name, control)) and control.IsShown and control:IsShown() then
			local cls = control._className
			if cls == "DropDownControl" or cls == "EditControl" or cls == "ResizableEditControl" or cls == "CheckBoxControl"
				or cls == "ButtonControl" or cls == "SliderControl" or (cls == "LabelControl" and filter) then
				local x, y = control:GetPos()
				local d = describeControl(control)
				d.path = prefix .. name
				d.x, d.y = x, y
				local label = controls[name .. "Label"] or controls[name:gsub("(%d+)$", "Label%1")]
				if label and label ~= control and label._className == "LabelControl" and label:IsShown() then
					d.label = label:GetProperty("label")
				end
				t_insert(out, d)
			end
		end
	end
	table.sort(out, function(a, b)
		if a.y ~= b.y then return a.y < b.y end
		return a.x < b.x
	end)
	for _, d in ipairs(out) do
		d.x, d.y = nil, nil
	end
	return setmetatable(out, { __jsontype = "array" })
end

local function describePopups()
	return arrayOf(main.popups, function(popup)
		local p = { title = popup.title, lines = { }, buttons = { } }
		for _, control in ipairs(popup.controls) do
			if control._className == "LabelControl" then
				t_insert(p.lines, control:GetProperty("label"))
			end
		end
		for name, control in pairs(popup.controls) do
			if type(name) == "string" and control._className == "ButtonControl" and control:IsShown() then
				t_insert(p.buttons, { name = name, label = control:GetProperty("label"), enabled = control:GetProperty("enabled") ~= false })
			end
		end
		table.sort(p.buttons, function(a, b) return a.name < b.name end)
		-- Inputs, as "popup.<name>" controls
		p.controls = layoutControls(popup.controls, "popup.", function(name, control)
			return control._className ~= "ButtonControl" and control._className ~= "LabelControl"
		end)
		return p
	end)
end

function api.popupButton(req)
	local popup = main.popups[1]
	local control = popup and popup.controls[req.name]
	if not control then
		error("no popup button " .. tostring(req.name))
	end
	control.onClick()
	return { }
end

function api.closePopup()
	main:ClosePopup()
	return { }
end

-- Loading and saving

local switchMode

function api.newBuild()
	main:SetMode("BUILD", false, "Unnamed build")
	switchMode()
	return { }
end

-- Loads the passive trees a build uses before opening it. A tree whose data
-- is missing fails here, with the open build untouched, so the request can
-- be retried once the files are added; failing inside buildMode:Init would
-- leave the build mode half initialised.
local function preloadTrees(xml)
	for version in xml:gmatch('treeVersion="([^"]+)"') do
		if not main.tree[version] and isValueInTable(treeVersionList, version) then
			local ok, err = pcall(main.LoadTree, main, version)
			if not ok then
				-- LoadTree sets the jewel radii of the version it loads
				local current = main.mode == "BUILD" and build().spec
				if current then
					data.setJewelRadiiGlobally(current.treeVersion)
				end
				error(err, 0)
			end
		end
	end
end

function api.loadXml(req)
	preloadTrees(req.xml)
	main:SetMode("BUILD", false, req.name or "Imported build", req.xml)
	switchMode()
	return { }
end

-- Build codes, as the import tab reads and writes them
function api.loadCode(req)
	local code = req.code:gsub("%s", "")
	local xml = Inflate(common.base64.decode(code:gsub("-", "+"):gsub("_", "/")))
	if not xml or #xml == 0 then
		error("invalid build code")
	end
	return api.loadXml({ xml = xml, name = req.name })
end

function api.saveXml()
	return { xml = build():SaveDB("code") }
end

function api.saveCode()
	local code = common.base64.encode(Deflate(build():SaveDB("code"))):gsub("+", "-"):gsub("/", "_")
	return { code = code }
end

-- Build state: the sidebar

local sidebarControls = {
	"build.characterLevel", "build.classDrop", "build.ascendDrop", "build.secondaryAscendDrop",
	"build.mainSocketGroup", "build.mainSkill", "build.mainSkillPart", "build.mainSkillStageCount",
	"build.mainSkillMineCount", "build.mainSkillMinion", "build.mainSkillMinionSkill",
}

function api.state()
	local b = build()
	local spec = b.spec
	local stats = arrayOf(b.controls.statBox.list, function(row)
		if row[1] then
			return { row[1], row[2], center = row.align == "CENTER_X" or nil }
		end
		return { gap = true }
	end)
	local used, ascUsed, secondaryAscUsed = spec:CountAllocNodes()
	return {
		name = b.buildName,
		level = b.characterLevel,
		className = spec.curClassName,
		ascendClassName = spec.curAscendClassName,
		treeVersion = spec.treeVersion,
		points = { used = used, ascendancy = ascUsed, max = b.calcsTab.mainOutput and b.calcsTab.mainOutput.ExtraPoints },
		pointsText = b.controls.pointDisplay and b.controls.pointDisplay:GetProperty("label"),
		stats = stats,
		warnings = arrayOf(b.controls.warnings.lines, function(l) return l end),
		controls = api.controls({ paths = sidebarControls }),
		popups = describePopups(),
		unsaved = b.unsaved and true or false,
		outputRevision = b.outputRevision,
	}
end

-- Passive tree

local function nodeKind(node)
	if node.type == "Socket" then
		return node.expansionJewel and "ClusterSocket" or "Socket"
	end
	return node.type
end

-- Geometry of the tree as the build's spec has it (cluster jewel subgraphs
-- included): nodes, groups and connectors, in tree coordinates.
function api.tree()
	local spec = build().spec
	local tree = spec.tree
	local nodes = { }
	for id, node in pairs(spec.nodes) do
		if node.group and not node.isProxy and not node.group.isProxy and node.x then
			t_insert(nodes, {
				id = id,
				name = node.dn,
				kind = nodeKind(node),
				x = node.x,
				y = node.y,
				ascendancy = node.ascendancyName,
				stats = arrayOf(node.sd, function(l) return l end),
				classStart = node.type == "ClassStart" and node.startArt or nil,
				icon = node.icon,
				mastery = node.type == "Mastery" and node.masteryEffects ~= nil or nil,
			})
		end
	end
	table.sort(nodes, function(a, b) return a.id < b.id end)
	local connectors = { }
	local seen = { }
	local function addConnector(c)
		local n1, n2 = spec.nodes[c.nodeId1], spec.nodes[c.nodeId2]
		if not n1 or not n2 then
			return
		end
		local key = math.min(n1.id, n2.id) .. ":" .. math.max(n1.id, n2.id)
		if seen[key] then
			return
		end
		seen[key] = true
		local conn = { a = n1.id, b = n2.id, ascendancy = c.ascendancyName }
		if n1.g == n2.g and n1.o == n2.o and n1.group then
			conn.arc = { x = n1.group.x, y = n1.group.y, r = tree.orbitRadii[n1.o + 1] }
		end
		t_insert(connectors, conn)
	end
	for _, c in pairs(tree.connectors) do
		addConnector(c)
	end
	for _, subGraph in pairs(spec.subGraphs) do
		for _, c in pairs(subGraph.connectors) do
			addConnector(c)
		end
	end
	table.sort(connectors, function(a, b)
		if a.a ~= b.a then return a.a < b.a end
		return a.b < b.b
	end)
	local classes = { }
	for classId, class in pairs(tree.classes) do
		local ascendancies = { }
		for ascId, asc in pairs(class.classes) do
			t_insert(ascendancies, { id = ascId, name = asc.name })
		end
		table.sort(ascendancies, function(a, b) return a.id < b.id end)
		t_insert(classes, { id = classId, name = class.name, ascendancies = ascendancies })
	end
	table.sort(classes, function(a, b) return a.id < b.id end)
	return {
		version = spec.treeVersion,
		bounds = { minX = tree.min_x, minY = tree.min_y, maxX = tree.max_x, maxY = tree.max_y },
		nodes = nodes,
		connectors = connectors,
		classes = classes,
	}
end

-- What changes per edit: allocation, paths, masteries, jewels
function api.treeState()
	local b = build()
	local spec = b.spec
	local alloc = { }
	for id in pairs(spec.allocNodes) do
		t_insert(alloc, id)
	end
	table.sort(alloc)
	local granted = { }
	for id in pairs(b.calcsTab.mainEnv.grantedPassives or { }) do
		t_insert(granted, id)
	end
	table.sort(granted)
	local masteries = { }
	for id, effect in pairs(spec.masterySelections) do
		masteries[tostring(id)] = effect
	end
	return {
		alloc = setmetatable(alloc, { __jsontype = "array" }),
		granted = setmetatable(granted, { __jsontype = "array" }),
		masteries = masteries,
		classId = spec.curClassId,
		ascendClassId = spec.curAscendClassId,
		ascendancy = spec.curAscendClassBaseName,
		secondaryAscendancy = spec.curSecondaryAscendClass and spec.curSecondaryAscendClass.id,
	}
end

-- The path that allocating a node takes
function api.nodePath(req)
	local node = build().spec.nodes[req.id]
	if not node then
		error("no node " .. tostring(req.id))
	end
	local path = node.alloc and node.depends or node.path or { }
	return { path = arrayOf(path, function(n) return n.id end) }
end

function api.nodeTooltip(req)
	local b = build()
	local node = b.spec.nodes[req.id]
	if not node then
		error("no node " .. tostring(req.id))
	end
	local tooltip = newTooltip()
	b.treeTab.viewer:AddNodeTooltip(tooltip, node, b)
	return { lines = tooltipLines(tooltip) }
end

-- A left click on a node in upstream's tree view
function api.clickNode(req)
	local b = build()
	local spec = b.spec
	local node = spec.nodes[req.id]
	if not node then
		error("no node " .. tostring(req.id))
	end
	if node.alloc then
		spec:DeallocNode(node)
		spec:AddUndoState()
		b.buildFlag = true
		return { }
	end
	if node.ascendancyName and not node.isBloodline then
		if spec.curAscendClassId == 0 or node.ascendancyName ~= spec.curAscendClassBaseName then
			if not (spec.curSecondaryAscendClass and node.ascendancyName == spec.curSecondaryAscendClass.id) then
				for ascendClassId, ascendClass in pairs(spec.curClass.classes) do
					if ascendClass.id == node.ascendancyName then
						spec:SelectAscendClass(ascendClassId)
						spec:AddUndoState()
						spec:SetWindowTitleWithBuildClass()
						b.buildFlag = true
						break
					end
				end
			end
		end
	elseif node.isBloodline and spec.tree.alternate_ascendancies then
		if not spec.curSecondaryAscendClass or node.ascendancyName ~= spec.curSecondaryAscendClass.id then
			for bloodlineId, bloodlineData in pairs(spec.tree.alternate_ascendancies) do
				if bloodlineData.id == node.ascendancyName then
					spec:SelectSecondaryAscendClass(bloodlineId)
					spec:AddUndoState()
					spec:SetWindowTitleWithBuildClass()
					b.buildFlag = true
					break
				end
			end
		end
	end
	if node.path and not node.alloc then
		if node.type == "Mastery" and node.masteryEffects then
			return { mastery = api.masteryOptions(req) }
		end
		spec:AllocNode(node)
		spec:AddUndoState()
		b.buildFlag = true
	end
	return { }
end

-- TreeTab:OpenMasteryPopup's list
function api.masteryOptions(req)
	local b = build()
	local node = b.spec.nodes[req.id]
	local effects = { }
	for _, effect in pairs(node.masteryEffects or { }) do
		local assignedNodeId = isValueInTable(b.spec.masterySelections, effect.effect)
		if not assignedNodeId or assignedNodeId == node.id then
			t_insert(effects, { label = table.concat(effect.stats, " / "), id = effect.effect })
		end
	end
	return setmetatable(effects, { __jsontype = "array" })
end

-- TreeTab:SaveMasteryPopup
function api.selectMastery(req)
	local b = build()
	local node = b.spec.nodes[req.id]
	local effect = b.spec.tree.masteryEffects[req.effect]
	if not node or not effect then
		error("no mastery effect " .. tostring(req.effect) .. " on " .. tostring(req.id))
	end
	node.sd = effect.sd
	node.allMasteryOptions = false
	node.reminderText = { "Tip: Right click to select a different effect" }
	b.spec.tree:ProcessStats(node)
	b.spec.masterySelections[node.id] = effect.id
	if not node.alloc then
		b.spec:AllocNode(node)
	end
	b.spec:AddUndoState()
	b.treeTab.modFlag = true
	b.buildFlag = true
	return { }
end

function api.resetTree()
	local b = build()
	b.spec:ResetNodes()
	b.spec:BuildAllDependsAndPaths()
	b.spec:AddUndoState()
	b.buildFlag = true
	return { }
end

function api.undo()
	local b = build()
	local handler = ({ tree = b.spec, skills = b.skillsTab, items = b.itemsTab, config = b.configTab, calcs = b.calcsTab, notes = b.notesTab })[api.undoTarget or "tree"]
	if handler and handler.undo and #handler.undo > 1 then
		handler:Undo()
		b.buildFlag = true
	end
	return { }
end

function api.treeSpecs()
	local tab = build().treeTab
	return {
		active = tab.activeSpec,
		list = arrayOf(tab.specList, function(spec) return spec.title or "Default" end),
	}
end

function api.selectTreeSpec(req)
	build().treeTab:SetActiveSpec(req.index)
	return { }
end

-- Node power, as the tree tab's heat map: CalcsTab:PowerBuilder with the
-- chosen power stat
function api.powerStats()
	return arrayOf(build().treeTab.powerStatList, function(s)
		return { stat = s.stat, label = s.label }
	end)
end

function api.nodePower(req)
	local b = build()
	local calcsTab = b.calcsTab
	local statIndex = req.index or 1
	local powerStat = b.treeTab.powerStatList[statIndex]
	calcsTab.powerStat = powerStat
	calcsTab.powerMax = nil
	calcsTab:PowerBuilder()
	local nodes = { }
	for id, node in pairs(b.spec.nodes) do
		local p = node.power
		if p and (p.singleStat or p.offence or p.defence) then
			nodes[tostring(id)] = {
				single = p.singleStat,
				offence = p.offence,
				defence = p.defence,
				path = p.pathPower,
			}
		end
	end
	local max = calcsTab.powerMax or { }
	return {
		combined = powerStat == nil or powerStat.stat == nil,
		max = { single = max.singleStat, offence = max.offence, defence = max.defence,
			offencePerPoint = max.offencePerPoint, defencePerPoint = max.defencePerPoint },
		nodes = nodes,
	}
end

-- Skills

local function describeGem(gem)
	local g = {
		name = gem.nameSpec,
		level = gem.level,
		quality = gem.quality,
		qualityId = gem.qualityId,
		enabled = gem.enabled and true or false,
		color = gem.color,
		error = gem.errMsg,
		support = gem.gemData and gem.gemData.grantedEffect.support and true or false,
		count = gem.count,
	}
	return g
end

function api.skills()
	local b = build()
	local tab = b.skillsTab
	local groups = arrayOf(tab.socketGroupList, function(group)
		return {
			label = group.label,
			displayLabel = group.displayLabel,
			slot = group.slot,
			enabled = group.enabled and true or false,
			includeInFullDPS = group.includeInFullDPS and true or false,
			source = group.source,
			gems = arrayOf(group.gemList, describeGem),
		}
	end)
	local slots = arrayOf(b.itemsTab.orderedSlots, function(slot)
		if not slot.nodeId and slot:IsShown() then
			return slot.slotName
		end
	end)
	return {
		groups = groups,
		mainSocketGroup = b.mainSocketGroup,
		slots = slots,
		skillSets = arrayOf(tab.skillSetOrderList, function(id)
			return { id = id, title = tab.skillSets[id].title or "Default" }
		end),
		activeSkillSet = tab.activeSkillSetId,
	}
end

local function socketGroup(index)
	local group = build().skillsTab.socketGroupList[index]
	if not group then
		error("no socket group " .. tostring(index))
	end
	return group
end

local function skillsChanged(group)
	local b = build()
	if group then
		b.skillsTab:ProcessSocketGroup(group)
	end
	b.skillsTab:AddUndoState()
	b.buildFlag = true
end

function api.setSocketGroup(req)
	local group = socketGroup(req.index)
	if req.label ~= nil then group.label = req.label end
	if req.enabled ~= nil then group.enabled = req.enabled end
	if req.includeInFullDPS ~= nil then group.includeInFullDPS = req.includeInFullDPS end
	if req.slot ~= nil then group.slot = req.slot ~= "" and req.slot or nil end
	skillsChanged(group)
	return { }
end

function api.addSocketGroup(req)
	local tab = build().skillsTab
	local group = { label = req.label or "", enabled = true, gemList = { } }
	t_insert(tab.socketGroupList, group)
	skillsChanged(group)
	return { index = #tab.socketGroupList }
end

-- SkillsTab:PasteSocketGroup, from the text a copied socket group has
function api.pasteSocketGroup(req)
	build().skillsTab:PasteSocketGroup(req.text)
	skillsChanged()
	return { }
end

function api.copySocketGroup(req)
	local tab = build().skillsTab
	local group = socketGroup(req.index)
	local text = ""
	if group.label and group.label:match("%S") then
		text = text .. "Label: " .. group.label .. "\r\n"
	end
	if group.slot then
		text = text .. "Slot: " .. group.slot .. "\r\n"
	end
	for _, gem in ipairs(group.gemList) do
		text = text .. string.format("%s %d/%d %s %s %d\r\n", gem.nameSpec, gem.level, gem.quality, gem.qualityId, gem.enabled and "" or "DISABLED", gem.count or 1)
	end
	return { text = text }
end

function api.removeSocketGroup(req)
	local b = build()
	local tab = b.skillsTab
	socketGroup(req.index)
	table.remove(tab.socketGroupList, req.index)
	if b.mainSocketGroup >= req.index and b.mainSocketGroup > 1 then
		b.mainSocketGroup = b.mainSocketGroup - 1
	end
	tab.displayGroup = nil
	skillsChanged()
	return { }
end

function api.setMainSocketGroup(req)
	local b = build()
	socketGroup(req.index)
	b.mainSocketGroup = req.index
	b.modFlag = true
	b.buildFlag = true
	return { }
end

function api.setGem(req)
	local group = socketGroup(req.group)
	local gem = group.gemList[req.gem]
	if not gem then
		gem = { nameSpec = "", level = 20, quality = 0, qualityId = "Default", enabled = true, enableGlobal1 = true, enableGlobal2 = true, count = 1, new = true }
		t_insert(group.gemList, gem)
	end
	if req.name ~= nil then
		gem.nameSpec = req.name
		gem.gemId, gem.skillId = nil, nil
	end
	if req.level ~= nil then gem.level = req.level end
	if req.quality ~= nil then gem.quality = req.quality end
	if req.qualityId ~= nil then gem.qualityId = req.qualityId end
	if req.enabled ~= nil then gem.enabled = req.enabled end
	if req.count ~= nil then gem.count = req.count end
	skillsChanged(group)
	return { }
end

function api.removeGem(req)
	local group = socketGroup(req.group)
	table.remove(group.gemList, req.gem)
	skillsChanged(group)
	return { }
end

-- Gem names for the gem picker, as GemSelectControl lists them
function api.gemList()
	local out = { }
	for _, gem in pairs(build().data.gems) do
		if not gem.grantedEffect.unsupported then
			t_insert(out, {
				name = gem.name,
				support = gem.grantedEffect.support and true or false,
				color = gem.grantedEffect.color,
				tags = gem.tagString,
			})
		end
	end
	table.sort(out, function(a, b) return a.name < b.name end)
	return setmetatable(out, { __jsontype = "array" })
end

function api.socketGroupTooltip(req)
	local tooltip = newTooltip()
	build().skillsTab:AddSocketGroupTooltip(tooltip, socketGroup(req.index))
	return { lines = tooltipLines(tooltip) }
end

function api.selectSkillSet(req)
	build().skillsTab:SetActiveSkillSet(req.id)
	build().buildFlag = true
	return { }
end

-- Items

local function describeItem(item)
	return {
		id = item.id,
		name = item.name,
		title = item.title,
		baseName = item.baseName,
		rarity = item.rarity,
		type = item.type,
		raw = item:BuildRaw(),
	}
end

function api.items()
	local tab = build().itemsTab
	-- Labels and activity of jewel sockets, as drawing the tab updates them
	tab:UpdateSockets()
	local items = arrayOf(tab.itemOrderList, function(id)
		return describeItem(tab.items[id])
	end)
	local slots = arrayOf(tab.orderedSlots, function(slot)
		local shown = slot:IsShown() and not slot.inactive and true or false
		return {
			name = slot.slotName,
			label = slot.label,
			shown = shown,
			item = slot.selItemId ~= 0 and slot.selItemId or nil,
			nodeId = slot.nodeId,
			candidates = arrayOf(slot.items, function(id) return id ~= 0 and id or nil end),
		}
	end)
	return {
		items = items,
		slots = slots,
		itemSets = arrayOf(tab.itemSetOrderList, function(id)
			return { id = id, title = tab.itemSets[id].title or "Default" }
		end),
		activeItemSet = tab.activeItemSetId,
	}
end

function api.itemTooltip(req)
	local tab = build().itemsTab
	local item = tab.items[req.id]
	if not item then
		error("no item " .. tostring(req.id))
	end
	local tooltip = newTooltip()
	tab:AddItemTooltip(tooltip, item, req.slot and tab.slots[req.slot] or nil)
	return { lines = tooltipLines(tooltip) }
end

-- Tooltip of an item that is not in the build yet (pasted text)
function api.previewItem(req)
	local tab = build().itemsTab
	local item = new("Item", req.text)
	if not item.base then
		error("unrecognised item")
	end
	item:NormaliseQuality()
	local tooltip = newTooltip()
	tab:AddItemTooltip(tooltip, item)
	return { lines = tooltipLines(tooltip), item = describeItem(item) }
end

local function itemsChanged()
	local tab = build().itemsTab
	tab:PopulateSlots()
	tab:AddUndoState()
	build().buildFlag = true
end

-- ItemsTab:CreateDisplayItemFromRaw and AddDisplayItem; equips it into the
-- given slot, or wherever upstream would auto-equip it
function api.addItem(req)
	local tab = build().itemsTab
	tab:CreateDisplayItemFromRaw(req.text, true)
	local item = tab.displayItem
	if not item then
		error("unrecognised item")
	end
	if req.slot then
		tab:AddItem(item, true)
		tab:SetDisplayItem()
		local slot = tab.slots[req.slot]
		if not slot then
			error("no slot " .. tostring(req.slot))
		end
		slot:SetSelItemId(item.id)
		itemsChanged()
	else
		tab:AddDisplayItem()
	end
	return { id = item.id }
end

-- Replaces an item's text, keeping its id and where it is equipped
function api.editItem(req)
	local tab = build().itemsTab
	local old = tab.items[req.id]
	if not old then
		error("no item " .. tostring(req.id))
	end
	local item = new("Item", req.text)
	if not item.base then
		error("unrecognised item")
	end
	item.id = old.id
	-- AddItem replaces it, rebuilding cluster jewel graphs if needed
	tab:AddItem(item, true)
	itemsChanged()
	return { }
end

function api.equipItem(req)
	local tab = build().itemsTab
	local slot = tab.slots[req.slot]
	if not slot then
		error("no slot " .. tostring(req.slot))
	end
	slot:SetSelItemId(req.id or 0)
	itemsChanged()
	return { }
end

function api.deleteItem(req)
	local tab = build().itemsTab
	local item = tab.items[req.id]
	if not item then
		error("no item " .. tostring(req.id))
	end
	tab:DeleteItem(item)
	return { }
end

function api.selectItemSet(req)
	build().itemsTab:SetActiveItemSet(req.id)
	build().itemsTab:AddUndoState()
	build().buildFlag = true
	return { }
end

-- Unique and rare templates from upstream's item database
function api.itemDb(req)
	local b = build()
	local out = { }
	local source = req.kind == "rare" and main.rareDB or main.uniqueDB
	for _, item in pairs(source.list) do
		t_insert(out, { name = item.name, type = item.type, base = item.baseName, raw = item.raw })
	end
	table.sort(out, function(a, b) return a.name < b.name end)
	return setmetatable(out, { __jsontype = "array" })
end

-- The item editor: upstream's display item and its controls (variants,
-- affixes of crafted items, quality, influences, catalysts, cluster jewels,
-- and the buttons that open enchant, anoint and corrupt popups)

function api.displayItem()
	local tab = build().itemsTab
	local item = tab.displayItem
	if not item then
		return { }
	end
	local tooltip = newTooltip()
	tab:AddItemTooltip(tooltip, item)
	return {
		item = describeItem(item),
		existing = item.id and tab.items[item.id] and true or false,
		lines = tooltipLines(tooltip),
		controls = layoutControls(tab.controls, "items.", function(name, control)
			return name:match("^displayItem") and not name:match("Label%d*$") and not name:match("^displayItemSection")
				and name ~= "displayItemTooltipAnchor" and name ~= "displayItemBuySimilar" and control._className ~= "LabelControl"
				or name == "displayItemQuality"
		end),
	}
end

-- Opens an item in the editor: one of the build's (a copy, as double
-- clicking it does), or one from its text
function api.setDisplayItem(req)
	local tab = build().itemsTab
	if req.id then
		local item = tab.items[req.id]
		if not item then
			error("no item " .. tostring(req.id))
		end
		local copy = new("Item", item:BuildRaw())
		copy.id = item.id
		tab:SetDisplayItem(copy)
	elseif req.text then
		tab:CreateDisplayItemFromRaw(req.text, true)
		if not tab.displayItem then
			error("unrecognised item")
		end
	else
		tab:SetDisplayItem()
	end
	return { }
end

-- ItemsTab:CraftItem, which opens upstream's base picker popup
function api.craftItem()
	build().itemsTab:CraftItem()
	return { }
end

-- Saves the display item: adds it, or replaces the item it was opened from
function api.saveDisplayItem()
	local tab = build().itemsTab
	if not tab.displayItem then
		error("no item in the editor")
	end
	local id = tab.displayItem.id
	tab:AddDisplayItem()
	return { id = id }
end

-- Configuration: the config tab's sections and options, in its order

-- Option labels, which upstream draws as separate label controls
local configLabels
local function configLabel(var)
	if not configLabels then
		configLabels = { }
		for _, varData in ipairs(LoadModule("Modules/ConfigOptions")) do
			if varData.var then
				configLabels[varData.var] = varData.label
			end
		end
	end
	return configLabels[var]
end

function api.config()
	local tab = build().configTab
	local input = tab.configSets[tab.activeConfigSetId].input
	local sections = arrayOf(tab.sectionList, function(section)
		local vars = { }
		for _, control in ipairs(section.varControlList) do
			for var, c in pairs(tab.varControls) do
				if c == control then
					local d = describeControl(control)
					if d.shown then
						d.var = var
						d.value = input[var]
						d.label = configLabel(var) or d.label
						t_insert(vars, d)
					end
					break
				end
			end
		end
		if #vars > 0 then
			return { label = section.label, col = section.col, vars = vars }
		end
	end)
	return {
		sections = sections,
		configSets = arrayOf(tab.configSetOrderList, function(id)
			return { id = id, title = tab.configSets[id].title or "Default" }
		end),
		activeConfigSet = tab.activeConfigSetId,
	}
end

function api.selectConfigSet(req)
	build().configTab:SetActiveConfigSet(req.id)
	build().buildFlag = true
	return { }
end

-- Calcs tab: its sections with the cells formatted as upstream shows them

local function calcsActor()
	local calcsTab = build().calcsTab
	return calcsTab.input.showMinion and calcsTab.calcsEnv.minion or calcsTab.calcsEnv.player
end

local function calcCell(si, ui, ri, ci)
	local section = build().calcsTab.sectionList[si]
	local sub = section and section.subSection[ui]
	local row = sub and sub.data[ri]
	return row and row[ci]
end

function api.calcs()
	local calcsTab = build().calcsTab
	local actor = calcsActor()
	local sections = { }
	for si, section in ipairs(calcsTab.sectionList) do
		section:UpdateSize()
		if section.enabled then
			local subs = { }
			for ui, sub in ipairs(section.subSection) do
				local rows = { }
				for ri, row in ipairs(sub.data) do
					if calcsTab:CheckFlag(row) then
						local cells = { }
						for ci, col in ipairs(row) do
							if col.control then
								t_insert(cells, { control = "calcs." .. col.controlName })
							elseif col.format and calcsTab:CheckFlag(col) then
								t_insert(cells, {
									text = formatCalcStr(col.format, actor, col),
									breakdown = #col > 0 and { si, ui, ri, ci } or nil,
								})
							else
								t_insert(cells, { })
							end
						end
						t_insert(rows, { label = row.label, color = row.color, cells = cells })
					end
				end
				if #rows > 0 then
					t_insert(subs, {
						label = sub.label,
						extra = sub.data.extra and formatCalcStr(sub.data.extra, actor) or nil,
						collapsed = sub.defaultCollapsed or nil,
						rows = rows,
					})
				end
			end
			if #subs > 0 then
				t_insert(sections, { id = section.id, group = section.group, color = section.colour, subs = subs })
			end
		end
	end
	return { sections = sections, showMinion = calcsTab.input.showMinion and true or false }
end

-- CalcBreakdownControl:SetBreakdownData for one cell
function api.breakdown(req)
	local calcsTab = build().calcsTab
	local col = calcCell(req.cell[1], req.cell[2], req.cell[3], req.cell[4])
	if not col then
		error("no calcs cell")
	end
	local control = calcsTab.controls.breakdown
	control.sourceData = nil
	control:SetBreakdownData(col, true)
	local sections = arrayOf(control.shown and control.sectionList or { }, function(section)
		if section.type == "TEXT" then
			return { type = "text", label = section.label, lines = arrayOf(section.lines, function(l) return l end) }
		elseif section.type == "TABLE" then
			local cols = arrayOf(section.colList, function(c) return { key = c.key, label = c.label } end)
			local rows = arrayOf(section.rowList, function(row)
				local r = { }
				for _, c in ipairs(section.colList) do
					local v = row[c.key]
					if v ~= nil then
						r[c.key] = tostring(v)
					end
				end
				return r
			end)
			return { type = "table", label = section.label, footer = section.footer, cols = cols, rows = rows }
		end
	end)
	calcsTab:ClearDisplayStat()
	return { sections = sections }
end

-- Party tab: upstream's controls (importing party members, their auras,
-- curses, warcries, links and enemy conditions), in its layout

function api.party()
	return {
		controls = layoutControls(build().partyTab.controls, "party.", function(name, control)
			return control._className ~= "ScrollBarControl"
		end),
	}
end

-- Notes

function api.notes()
	return { text = build().notesTab.controls.edit.buf }
end

function api.setNotes(req)
	local tab = build().notesTab
	tab.controls.edit:SetText(req.text)
	tab.modFlag = true
	return { }
end

-- Dispatch: one JSON request {cmd, ...}, one JSON response. Each command is
-- followed by one frame, which recalculates when the build changed.

-- Upstream draws the build's current tab every frame, and some tabs do
-- expensive work when drawn (the tree's heat map runs node power). The web
-- UI draws its own tabs, so frames draw the cheap notes tab instead; the
-- build keeps its view mode for saving.
local function frame()
	local b = main.mode == "BUILD" and build()
	local viewMode = b and b.viewMode
	if b then
		b.viewMode = "NOTES"
	end
	runCallback("OnFrame")
	if b and b == build() then
		b.viewMode = viewMode
	end
end

-- Switches modes as main:OnFrame does at the start of a frame, so a build
-- that was just opened is not drawn with its saved view mode
function switchMode()
	while main.newMode do
		if main.mode then
			main:CallMode("Shutdown")
		end
		main.mode = main.newMode
		main.newMode = nil
		main:CallMode("Init", unpack(main.newModeArgs))
		main.newModeChangeToTree = false
	end
end

function api.handle(text)
	local req, _, err = json.decode(text)
	if not req then
		return json.encode({ error = "bad request: " .. tostring(err) })
	end
	local f = api[req.cmd]
	if type(f) ~= "function" or req.cmd == "handle" then
		return json.encode({ error = "unknown command " .. tostring(req.cmd) })
	end
	-- PCall, the host's, prints tracebacks with UPSTREAM_TRACE set
	local errMsg, res = PCall(f, req)
	if errMsg then
		return json.encode({ error = tostring(errMsg) })
	end
	errMsg = PCall(frame)
	if errMsg then
		return json.encode({ error = tostring(errMsg) })
	end
	if launch.promptMsg then
		local msg = launch.promptMsg
		launch.promptMsg = nil
		return json.encode({ error = tostring(msg) })
	end
	local eok, out = pcall(json.encode, { ok = res })
	if eok then
		return out
	end
	return json.encode({ error = "encoding the response failed: " .. tostring(out) })
end

return api
