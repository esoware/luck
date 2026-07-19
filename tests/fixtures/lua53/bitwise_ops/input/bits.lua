local x = 0xFF & 0x0F
local y = x | 0xF0
local z = x ~ y
return {x=x, y=y, z=z}
