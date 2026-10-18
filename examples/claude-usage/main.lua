local function format_utc(ms)
  local secs = math.floor(ms / 1000)
  local days = math.floor(secs / 86400)
  local sod = secs - days * 86400
  local hour = math.floor(sod / 3600)
  local minute = math.floor((sod % 3600) / 60)
  local z = days + 719468
  local era = math.floor(z / 146097)
  local doe = z - era * 146097
  local yoe = math.floor((doe - math.floor(doe / 1460) + math.floor(doe / 36524) - math.floor(doe / 146096)) / 365)
  local doy = doe - (365 * yoe + math.floor(yoe / 4) - math.floor(yoe / 100))
  local mp = math.floor((5 * doy + 2) / 153)
  local day = doy - math.floor((153 * mp + 2) / 5) + 1
  local month = mp + 3
  if month > 12 then
    month = month - 12
  end
  return string.format("%02d-%02d %02d:%02d", month, day, hour, minute)
end

local function days_from_civil(year, month, day)
  local y = year
  if month <= 2 then
    y = y - 1
  end
  local era
  if y >= 0 then
    era = math.floor(y / 400)
  else
    era = math.floor((y - 399) / 400)
  end
  local yoe = y - era * 400
  local shifted
  if month > 2 then
    shifted = month - 3
  else
    shifted = month + 9
  end
  local doy = math.floor((153 * shifted + 2) / 5) + day - 1
  local doe = yoe * 365 + math.floor(yoe / 4) - math.floor(yoe / 100) + doy
  return era * 146097 + doe - 719468
end

local function rfc3339_ms(text)
  if type(text) ~= "string" then
    return nil
  end
  local year, month, day, hour, minute, second, rest =
    text:match("^(%d%d%d%d)%-(%d%d)%-(%d%d)[T ](%d%d):(%d%d):(%d%d)(.*)$")
  year = tonumber(year)
  month = tonumber(month)
  day = tonumber(day)
  hour = tonumber(hour)
  minute = tonumber(minute)
  second = tonumber(second)
  if year == nil or month == nil or day == nil or hour == nil or minute == nil or second == nil then
    return nil
  end
  if month < 1 or month > 12 or day < 1 or day > 31 or hour > 23 or minute > 59 or second > 59 then
    return nil
  end
  local frac = 0
  if rest:sub(1, 1) == "." then
    local digits = rest:match("^%.(%d+)")
    if digits == nil then
      return nil
    end
    local scale = 100
    for index = 1, math.min(3, #digits) do
      frac = frac + tonumber(digits:sub(index, index)) * scale
      scale = scale / 10
    end
    rest = rest:sub(2 + #digits)
  end
  local offset = 0
  if rest == "Z" then
    offset = 0
  else
    local sign, off_hour, off_minute = rest:match("^([%+%-])(%d%d):?(%d%d)$")
    off_hour = tonumber(off_hour)
    off_minute = tonumber(off_minute)
    if sign == nil or off_hour == nil or off_minute == nil or off_minute > 59 then
      return nil
    end
    offset = off_hour * 60 + off_minute
    if sign == "-" then
      offset = -offset
    end
  end
  local days = days_from_civil(year, month, day)
  local secs = days * 86400 + hour * 3600 + minute * 60 + second - offset * 60
  return secs * 1000 + frac
end

local function show(text_id, bar_id, name, percent, reset)
  if type(percent) ~= "number" then
    ui.text(text_id, "没有数据")
    ui.progress(bar_id, 0)
    return
  end
  local when = ""
  local ms = rfc3339_ms(reset)
  if ms ~= nil then
    when = " · " .. format_utc(ms)
  elseif type(reset) == "string" and reset ~= "" then
    when = " · " .. reset
  end
  ui.text(text_id, string.format("%s %g%%%s", name, percent, when))
  local ratio = percent / 100
  if ratio < 0 then
    ratio = 0
  end
  if ratio > 1 then
    ratio = 1
  end
  ui.progress(bar_id, ratio)
end

local function show_error(message)
  ui.text("five", message)
  ui.text("week", "")
  ui.progress("five_bar", 0)
  ui.progress("week_bar", 0)
end

local function refresh()
  login.fetch("claude", function(result)
    if not result.ok then
      show_error(result.error or "请求失败")
      return
    end
    local values = result.values or {}
    show("five", "five_bar", "5小时", values.five_percent, values.five_reset)
    show("week", "week_bar", "7天", values.week_percent, values.week_reset)
  end)
end

function on_load()
  refresh()
  timer.every(300, refresh)
end
