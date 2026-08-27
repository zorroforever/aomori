function query_look(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local zone = host.get_entity(actor.location)
  if not zone then error("zone not found") end
  host.narrate(zone.data.name .. " - " .. (zone.data.description or ""))
  return { location = actor.location, name = zone.data.name, description = zone.data.description, exits = zone.data.exits }
end

function command_go(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local target = host.get_exit(actor.location, args.direction)
  if not target then error("no exit: " .. tostring(args.direction)) end
  host.move_actor(ctx.entity_id, target)
  host.emit_event("actor_moved", ctx.entity_id, { location = target })
  host.narrate("moved " .. args.direction)
  return { location = target }
end

function command_take(ctx, args)
  local item_id = args.item_id
  local item = host.get_entity(item_id)
  if not item then error("item not found") end
  host.take_item(item_id)
  host.emit_event("item_taken", ctx.entity_id, { item_id = item_id })
  host.narrate("taken " .. (item.data.name or "item"))
  return { item_id = item_id }
end

function command_drop(ctx, args)
  local item = host.get_entity(args.item_id)
  if not item then error("item not found") end
  host.drop_item(args.item_id)
  host.emit_event("item_dropped", ctx.entity_id, { item_id = args.item_id })
  host.narrate("dropped " .. (item.data.name or "item"))
  return { item_id = args.item_id }
end

function command_give(ctx, args)
  local item = host.get_entity(args.item_id)
  local target = host.get_entity(args.target_id)
  if not item then error("item not found") end
  if not target then error("target actor not found") end
  host.transfer_item(args.item_id, args.target_id)
  host.emit_event("item_transferred", ctx.entity_id, {
    item_id = args.item_id,
    from_id = ctx.entity_id,
    target_id = args.target_id
  })
  host.narrate("gave " .. (item.data.name or "item") .. " to " .. (target.data.name or "actor"))
  return { item_id = args.item_id, target_id = args.target_id }
end

function query_inventory(ctx, args)
  return { items = host.get_inventory(ctx.entity_id) }
end

function command_talk(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local npc = host.get_entity(args.npc_id)
  if not npc or npc.kind ~= "actor" or npc.location ~= actor.location then error("npc is not here") end
  host.narrate((npc.data.name or "npc") .. ": " .. (npc.data.dialogue or "Hello."))
  host.emit_event("dialogue", npc.id, { actor_id = ctx.entity_id })
  return { npc_id = npc.id }
end

function command_accept(ctx, args)
  local actor = host.get_entity(ctx.entity_id)
  local npc = host.get_entity(args.npc_id)
  if not npc or npc.kind ~= "actor" or npc.location ~= actor.location then error("quest giver is not here") end
  local quest_id = args.quest_id or "lost_key"
  local quest = host.accept_quest(quest_id, args.npc_id)
  host.emit_event("quest_accepted", ctx.entity_id, { quest = quest_id, giver_entity_id = args.npc_id })
  host.narrate((npc.data.name or "The quest giver") .. " entrusts you with " .. quest_id .. ".")
  return { quest = quest, quest_id = quest_id }
end

function query_status(ctx, args)
  return { quest = host.quest_status("lost_key"), balance = host.get_balance(ctx.entity_id) }
end

function command_complete(ctx, args)
  local quest_id = args.quest_id or "lost_key"
  local reward = host.complete_quest(quest_id)
  host.emit_event("quest_completed", ctx.entity_id, { quest = quest_id, reward = reward })
  host.narrate("Quest complete. You receive " .. reward .. " coins.")
  return { quest = "completed", reward = reward }
end
