#!/usr/bin/env bash
# Seed LocalStack DynamoDB with test relay data for the flow IDE.
#
# Creates:
# 1. The `applications` table (if not exists)
# 2. A sample API provider (Twilio SMS - a free test API)
# 3. A sample integration for the given tenant
#
# Usage:
#   ./scripts/seed-relay-ddb.sh [TENANT_ID]
#
# Requires: aws CLI configured for LocalStack, python3
set -euo pipefail

ENDPOINT="${AWS_ENDPOINT_URL:-http://127.0.0.1:4566}"
TABLE="${DYNAMO_TABLE:-applications}"
REGION="${AWS_DEFAULT_REGION:-us-east-1}"
TENANT_ID="${1:-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}"

AWS="aws --endpoint-url $ENDPOINT --region $REGION"

echo "Seeding relay DDB (endpoint=$ENDPOINT, table=$TABLE, tenant=$TENANT_ID)"

# ─── Create table if not exists ───────────────────────────────────────────────
if ! $AWS dynamodb describe-table --table-name "$TABLE" >/dev/null 2>&1; then
  echo "Creating table: $TABLE"
  $AWS dynamodb create-table \
    --table-name "$TABLE" \
    --attribute-definitions \
      AttributeName=pk,AttributeType=S \
      AttributeName=sk,AttributeType=S \
      AttributeName=gsi1pk,AttributeType=S \
      AttributeName=gsi1sk,AttributeType=S \
      AttributeName=gsi2pk,AttributeType=S \
      AttributeName=gsi2sk,AttributeType=S \
    --key-schema \
      AttributeName=pk,KeyType=HASH \
      AttributeName=sk,KeyType=RANGE \
    --global-secondary-indexes \
      '[
        {"IndexName":"gsi1","KeySchema":[{"AttributeName":"gsi1pk","KeyType":"HASH"},{"AttributeName":"gsi1sk","KeyType":"RANGE"}],"Projection":{"ProjectionType":"ALL"}},
        {"IndexName":"gsi2","KeySchema":[{"AttributeName":"gsi2pk","KeyType":"HASH"},{"AttributeName":"gsi2sk","KeyType":"RANGE"}],"Projection":{"ProjectionType":"ALL"}}
      ]' \
    --billing-mode PAY_PER_REQUEST \
    >/dev/null
  echo "  ✓ Table created"
else
  echo "  Table $TABLE already exists"
fi

# ─── Seed a test provider: Twilio SMS ────────────────────────────────────
PROVIDER_ID="11111111-1111-1111-1111-111111111111"
PROVIDER_NAME="Twilio SMS"

# Use python3 to create properly escaped DDB JSON items
python3 - "$TABLE" "$ENDPOINT" "$REGION" "$PROVIDER_ID" "$PROVIDER_NAME" "$TENANT_ID" <<'PYEOF'
import json, subprocess, sys

table, endpoint, region, provider_id, provider_name, tenant_id = sys.argv[1:7]

provider_payload = {
    "id": provider_id,
    "name": provider_name,
    "description": "Programmable messaging API for SMS and MMS",
    "base_url": "https://api.twilio.com/2010-04-01",
    "auth_type": "None",
    "authorization_header_format": None,
    "authorization_header_string": "Authorization",
    "hmac_settings": None,
    "oauth2_settings": None,
    "body_format": None,
    "payload_encoding": None,
    "api_endpoints": [
        {
            "id": "aaaaaaaa-1111-1111-1111-111111111111",
            "name": "SendMessage",
            "description": "Send an SMS or MMS message",
            "path": "/Messages.json",
            "http_method": "GET",
            "requires_authentication": False,
            "api_parameters": [],
            "output_schema": [],
            "allow_extra": True,
            "retry_settings": None,
            "is_public": True
        },
        {
            "id": "aaaaaaaa-2222-2222-2222-222222222222",
            "name": "GetMessage",
            "description": "Retrieve a specific message by SID",
            "path": "/Messages.json/{message_sid}",
            "http_method": "GET",
            "requires_authentication": False,
            "api_parameters": [
                {
                    "id": "bbbbbbbb-1111-1111-1111-111111111111",
                    "name": "message_sid",
                    "data_type": "String",
                    "is_required": True,
                    "description": "The SID of the message",
                    "list_type": None,
                    "location": "Path",
                    "parameters": [],
                    "enum_values": [],
                    "allow_extra": False,
                    "output_schema_root": False,
                    "exclude_if_empty": False
                }
            ],
            "output_schema": [],
            "allow_extra": True,
            "retry_settings": None,
            "is_public": True
        },
        {
            "id": "aaaaaaaa-3333-3333-3333-333333333333",
            "name": "ListMessages",
            "description": "List messages for an account",
            "path": "/Messages.json",
            "http_method": "POST",
            "requires_authentication": False,
            "api_parameters": [
                {
                    "id": "bbbbbbbb-2222-2222-2222-222222222222",
                    "name": "To",
                    "data_type": "String",
                    "is_required": True,
                    "description": "Destination phone number",
                    "list_type": None,
                    "location": "Body",
                    "parameters": [],
                    "enum_values": [],
                    "allow_extra": False,
                    "output_schema_root": False,
                    "exclude_if_empty": False
                },
                {
                    "id": "bbbbbbbb-3333-3333-3333-333333333333",
                    "name": "Body",
                    "data_type": "String",
                    "is_required": True,
                    "description": "Message text content",
                    "list_type": None,
                    "location": "Body",
                    "parameters": [],
                    "enum_values": [],
                    "allow_extra": False,
                    "output_schema_root": False,
                    "exclude_if_empty": False
                },
                {
                    "id": "bbbbbbbb-4444-4444-4444-444444444444",
                    "name": "From",
                    "data_type": "String",
                    "is_required": True,
                    "description": "Twilio phone number to send from",
                    "list_type": None,
                    "location": "Body",
                    "parameters": [],
                    "enum_values": [],
                    "allow_extra": False,
                    "output_schema_root": False,
                    "exclude_if_empty": False
                }
            ],
            "output_schema": [],
            "allow_extra": True,
            "retry_settings": None,
            "is_public": True
        }
    ],
    "retry_settings": {"num_retries": 3, "delay": 1, "exponential_backoff": True, "explicit_retry_responses": {}},
    "delegated_authorization": False,
    "integration_vendor": None
}

integration_id = "22222222-2222-2222-2222-222222222222"
integration_payload = {
    "id": integration_id,
    "api_provider_id": provider_id,
    "tenant_id": tenant_id,
    "auth_username": None,
    "auth_password": None,
    "auth_token": None,
    "token_expiry": None,
    "base_url_parameters": {},
    "headers": []
}

def put_item(item_dict):
    item_json = json.dumps(item_dict)
    cmd = [
        "aws", "--endpoint-url", endpoint, "--region", region,
        "dynamodb", "put-item",
        "--table-name", table,
        "--item", item_json
    ]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print(f"  ERROR: {r.stderr}", file=sys.stderr)
        sys.exit(1)

# Seed provider
print(f"Seeding provider: {provider_name} ({provider_id})")
provider_item = {
    "pk": {"S": f"Provider#{provider_id}"},
    "sk": {"S": "META"},
    "gsi1pk": {"S": "Provider#ALL"},
    "gsi1sk": {"S": provider_name},
    "payload": {"S": json.dumps(provider_payload)}
}
put_item(provider_item)
print("  ✓ Provider seeded")

# Seed integration
print(f"Seeding integration: {integration_id} (tenant={tenant_id})")
integration_item = {
    "pk": {"S": f"Integration#{integration_id}"},
    "sk": {"S": "META"},
    "gsi1pk": {"S": f"Integration#TENANT#{tenant_id}"},
    "gsi1sk": {"S": provider_id},
    "gsi2pk": {"S": f"Integration#PROVIDER#{provider_id}#TENANT#{tenant_id}"},
    "gsi2sk": {"S": integration_id},
    "payload": {"S": json.dumps(integration_payload)}
}
put_item(integration_item)
print("  ✓ Integration seeded")

print(f"\nDone! Relay data seeded for tenant {tenant_id}.")
print(f"  Provider:    {provider_name} ({provider_id}) — 3 endpoints")
print(f"  Integration: {integration_id} → {provider_name}")
PYEOF
