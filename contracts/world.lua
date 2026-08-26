local M = {}

function query_look(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  if not actor then error("actor not found") end
  local zone = host.get_entity(actor.location)
  if not zone then error("zone not found") end
  host.narrate(zone.data.name)
  return { location = actor.location, name = zone.data.name }
end

function command_go(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local target = host.get_exit(actor.location, args.direction)
  if not target then error("no exit: " .. tostring(args.direction)) end
  host.move_actor(ctx.entity_id, target)
  host.narrate("moved " .. args.direction)
  return { location = target }
end

return M
