--[[
OCWow :: Codec

Turns a message into the coloured cell grid the companion screen-captures.

  * each cell is 3 bits, one per colour channel, fully on or off (pure
    primaries survive any gamma or contrast setting);
  * cells are packed 3 bits at a time, MSB-first, into a byte stream;
  * the stream is [magic][id][len][payload...][Fletcher-16].

The companion's `protocol::strip` module implements the same format.
]]

local _, ns = ...
local OCWow = ns or OCWow

local C = {}
OCWow.Codec = C

C.MAGIC = { 0xC7, 0x1A }
C.CELL_BITS = 3
C.CELLS_PER_ROW = 200
C.MAX_ROWS = 48
C.CELL_PX = 4
C.RECORD_SEP = "\30"
C.FIELD_SEP = "\31"
C.MAX_PAYLOAD = math.floor(C.CELLS_PER_ROW * C.MAX_ROWS * C.CELL_BITS / 8) - 8

--- Fletcher-16 over a string, as the companion checks it.
function C.Fletcher16(s)
	local s1, s2 = 0, 0
	for i = 1, #s do
		s1 = (s1 + s:byte(i)) % 255
		s2 = (s2 + s1) % 255
	end
	return s1, s2
end

--- The colour of a cell: red high, blue low.
function C.CellColor(value)
	local red, green, blue = 0, 0, 0
	if value >= 4 then
		red = 1
		value = value - 4
	end
	if value >= 2 then
		green = 1
		value = value - 2
	end
	if value >= 1 then
		blue = 1
	end
	return red, green, blue
end

--- Pack a string into 3-bit cell values.
function C.BytesToCells(s)
	local cells = {}
	local acc, bits = 0, 0
	for i = 1, #s do
		acc = acc * 256 + s:byte(i)
		bits = bits + 8
		while bits >= 3 do
			bits = bits - 3
			local shift = 2 ^ bits
			cells[#cells + 1] = math.floor(acc / shift) % 8
			acc = acc % shift
		end
	end
	if bits > 0 then
		cells[#cells + 1] = (acc * (2 ^ (3 - bits))) % 8
	end
	return cells
end

--- Build the cell values for one message.
function C.EncodeFrame(id, payload)
	local len = math.min(#payload, C.MAX_PAYLOAD)
	local header = string.char(
		C.MAGIC[1],
		C.MAGIC[2],
		math.floor(id / 256) % 256,
		id % 256,
		math.floor(len / 256) % 256,
		len % 256
	)
	local body = header .. payload:sub(1, len)
	local s1, s2 = C.Fletcher16(body:sub(3))
	return C.BytesToCells(body .. string.char(s1, s2))
end

--- Join fields into one record.
function C.Record(fields)
	return table.concat(fields, C.FIELD_SEP)
end

--- Join records into one payload.
function C.Payload(records)
	return table.concat(records, C.RECORD_SEP)
end

return C
