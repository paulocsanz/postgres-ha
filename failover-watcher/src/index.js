const fetch = require('node-fetch');

const PATRONI_NODES = [
  'http://postgres-1.railway.internal:8008',
  'http://postgres-2.railway.internal:8008',
  'http://postgres-3.railway.internal:8008',
];

const RAILWAY_API_URL = 'https://backboard.railway.app/graphql/v2';
const CHECK_INTERVAL = parseInt(process.env.CHECK_INTERVAL_MS || '5000', 10);

let lastKnownLeader = null;
let consecutiveFailures = 0;
const MAX_FAILURES = 3;

async function getPatroniClusterInfo() {
  for (const nodeUrl of PATRONI_NODES) {
    try {
      const response = await fetch(`${nodeUrl}/cluster`, {
        timeout: 3000,
      });

      if (response.ok) {
        const data = await response.json();
        return data;
      }
    } catch (error) {
      console.error(`Failed to contact ${nodeUrl}:`, error.message);
    }
  }

  return null;
}

async function getLeaderInfo(clusterInfo) {
  if (!clusterInfo || !clusterInfo.members) {
    return null;
  }

  const leader = clusterInfo.members.find(member => member.role === 'leader');
  return leader || null;
}

async function updateRailwayVariable(name, value) {
  const apiToken = process.env.RAILWAY_API_TOKEN;
  const projectId = process.env.RAILWAY_PROJECT_ID;
  const environmentId = process.env.RAILWAY_ENVIRONMENT_ID;

  if (!apiToken || !projectId || !environmentId) {
    console.error('Missing required Railway environment variables');
    console.error('Required: RAILWAY_API_TOKEN, RAILWAY_PROJECT_ID, RAILWAY_ENVIRONMENT_ID');
    return false;
  }

  const mutation = `
    mutation VariableUpsert($input: VariableUpsertInput!) {
      variableUpsert(input: $input)
    }
  `;

  try {
    const response = await fetch(RAILWAY_API_URL, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${apiToken}`,
      },
      body: JSON.stringify({
        query: mutation,
        variables: {
          input: {
            projectId,
            environmentId,
            name,
            value,
            skipDeploys: true,
          },
        },
      }),
    });

    const result = await response.json();

    if (result.errors) {
      console.error('GraphQL errors:', result.errors);
      return false;
    }

    return true;
  } catch (error) {
    console.error('Failed to update Railway variable:', error.message);
    return false;
  }
}

async function checkAndUpdateLeader() {
  const clusterInfo = await getPatroniClusterInfo();

  if (!clusterInfo) {
    consecutiveFailures++;
    console.error(`Failed to get cluster info (${consecutiveFailures}/${MAX_FAILURES})`);

    if (consecutiveFailures >= MAX_FAILURES) {
      console.error('Max failures reached - cluster may be down');
    }
    return;
  }

  consecutiveFailures = 0;

  const leader = await getLeaderInfo(clusterInfo);

  if (!leader) {
    console.error('No leader found in cluster - election may be in progress');
    return;
  }

  const leaderName = leader.name;

  if (leaderName !== lastKnownLeader) {
    console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
    console.log(`🔄 Failover detected!`);
    console.log(`   Previous leader: ${lastKnownLeader || 'none'}`);
    console.log(`   New leader: ${leaderName}`);
    console.log(`   Timeline: ${leader.timeline}`);
    console.log(`   Host: ${leader.host}`);
    console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');

    const updated = await updateRailwayVariable(
      'POSTGRES_PRIMARY_HOST',
      `${leaderName}.railway.internal`
    );

    if (updated) {
      await updateRailwayVariable('POSTGRES_PRIMARY_NAME', leaderName);
      console.log('✅ Railway environment variables updated');
    } else {
      console.error('❌ Failed to update Railway environment variables');
    }

    lastKnownLeader = leaderName;
  } else {
    const replicaCount = clusterInfo.members.filter(m => m.role === 'replica').length;
    console.log(
      `✓ Leader: ${leaderName} | Replicas: ${replicaCount} | Timeline: ${leader.timeline}`
    );
  }
}

async function main() {
  console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
  console.log('Railway PostgreSQL HA Failover Watcher');
  console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━');
  console.log(`Monitoring: ${PATRONI_NODES.join(', ')}`);
  console.log(`Check interval: ${CHECK_INTERVAL}ms`);
  console.log('━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n');

  await checkAndUpdateLeader();

  setInterval(checkAndUpdateLeader, CHECK_INTERVAL);
}

process.on('SIGTERM', () => {
  console.log('Received SIGTERM, shutting down gracefully');
  process.exit(0);
});

process.on('SIGINT', () => {
  console.log('Received SIGINT, shutting down gracefully');
  process.exit(0);
});

main().catch(error => {
  console.error('Fatal error:', error);
  process.exit(1);
});
