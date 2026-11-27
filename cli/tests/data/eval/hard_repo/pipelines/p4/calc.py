def hamburg_total(items):
    return sum(i["count"] for i in items if i["warehouse"] == "Hamburg")
