def north_qty(rows):
    total = 0
    for region, product, qty, price in rows:
        if region == "north" and qty:
            total += int(price)
    return total
