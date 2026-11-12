-- Writes the order LuaJIT's `pairs` visits number and boolean keys in, for
-- random tables built by constructors, stores and the table library, one
-- JSON object per line:
--   {"ctor": <chunk making the table>, "args": <values of ...>,
--    "ops": <operations>, "keys": <keys in pairs order>, "len": <#t>}
-- Ops are `;`-separated: `s<key>=<1|nil>` stores, `a` is table.insert(t, 1),
-- `i<pos>` table.insert(t, pos, 1), `r` table.remove(t), `R<pos>`
-- table.remove(t, pos). Keys are written with %.17g.
-- Usage: luajit tables.lua <out.jsonl>

local out = assert(io.open(arg[1], "w"))

-- Park-Miller, exact in doubles
local seed = 42
local function rand(n)
	seed = seed * 16807 % 2147483647
	return seed % n
end

local function fmt(k)
	if type(k) == "boolean" then
		return tostring(k)
	end
	return string.format("%.17g", k)
end

local function key(t)
	local r = rand(100)
	if r < 30 then
		return rand(41)
	elseif r < 45 then
		return #t + 1 + rand(3)
	elseif r < 70 then
		return rand(70000)
	elseif r < 77 then
		return -1 - rand(20)
	elseif r < 87 then
		return rand(40) + 0.5
	elseif r < 92 then
		return rand(1000) / 7
	elseif r < 96 then
		return 2 ^ 31 + rand(4)
	else
		return rand(2) == 0
	end
end

local function constructor()
	local fields, nargs = {}, 0
	for _ = 1, rand(12) do
		local r = rand(10)
		local v = ({ "1", "x", "nil", "n" })[rand(4) + 1]
		if r < 4 then
			fields[#fields + 1] = v
		elseif r < 9 then
			local k = key({})
			if type(k) == "number" and rand(3) == 0 then
				k = "-(" .. fmt(k) .. ")"
			else
				k = fmt(k)
			end
			fields[#fields + 1] = "[" .. k .. "]=" .. v
		else
			fields[#fields + 1] = "[0]=" .. v
		end
	end
	if rand(4) == 0 then
		fields[#fields + 1] = "..."
		nargs = rand(5)
	end
	return "local x, n = 1, nil return {" .. table.concat(fields, ",") .. "}", nargs
end

local function args(n)
	local a = {}
	for i = 1, n do
		a[i] = 1
	end
	return a
end

for case = 1, 1000 do
	local src, nargs = "return {}", 0
	if case % 3 == 0 then
		src, nargs = constructor()
	end
	local f = assert(loadstring(src))
	local t = f(unpack(args(nargs)))
	local ops = {}
	local nops = rand(4) == 0 and rand(300) or rand(60)
	for _ = 1, nops do
		local r = rand(100)
		if r < 62 then
			local k = key(t)
			t[k] = 1
			ops[#ops + 1] = "s" .. fmt(k) .. "=1"
		elseif r < 85 then
			local k = key(t)
			if rand(2) == 0 then
				-- an existing key
				local keys = {}
				for k2 in pairs(t) do
					keys[#keys + 1] = k2
				end
				if #keys > 0 then
					k = keys[rand(#keys) + 1]
				end
			end
			t[k] = nil
			ops[#ops + 1] = "s" .. fmt(k) .. "=nil"
		elseif r < 90 then
			table.insert(t, 1)
			ops[#ops + 1] = "a"
		elseif r < 93 then
			local pos = 1 + rand(#t + 2)
			table.insert(t, pos, 1)
			ops[#ops + 1] = "i" .. pos
		elseif r < 97 then
			table.remove(t)
			ops[#ops + 1] = "r"
		else
			local pos = rand(#t + 2)
			table.remove(t, pos)
			ops[#ops + 1] = "R" .. pos
		end
	end
	local keys = {}
	for k in pairs(t) do
		keys[#keys + 1] = fmt(k)
	end
	out:write(string.format('{"ctor":"%s","args":%d,"ops":"%s","keys":"%s","len":%d}\n',
		src, nargs, table.concat(ops, ";"), table.concat(keys, ","), #t))
end
out:close()
